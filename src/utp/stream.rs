
use async_std::sync::{Arc, channel, Sender, Receiver, RwLock};
use async_std::task;
use async_std::io::{ErrorKind, Error};
use async_std::net::{SocketAddr, UdpSocket, SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr};
use futures::task::{Context, Poll};
use futures::{future, pin_mut};
use futures::future::FutureExt;
use hashbrown::HashMap;
use fixed::types::I48F16;





use crate::utils::Map;
use shared_arena::{ArenaBox, SharedArena};

use super::{
    UtpError, Result, SequenceNumber, Packet,
    Timestamp, PacketType, ConnectionId, HEADER_SIZE,
    SelectiveAckBit, DelayHistory,
    UDP_IPV4_MTU, UDP_IPV6_MTU, PACKET_MAX_SIZE
};
use super::{
    INIT_CWND, MSS,
    State as UtpState,
    TARGET, MIN_CWND
};
use super::tick::Tick;
use crate::utils::FromSlice;

use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, Ordering};

#[derive(Debug)]
struct State {
    utp_state: AtomicU8,
    recv_id: AtomicU16,
    send_id: AtomicU16,
    ack_number: AtomicU16,
    seq_number: AtomicU16,
    remote_window: AtomicU32,
    cwnd: AtomicU32,
    in_flight: AtomicU32,

    /// Packets sent but we didn't receive an ack for them
    inflight_packets: RwLock<Map<SequenceNumber, ArenaBox<Packet>>>,
}

impl State {
    async fn add_packet_inflight(&self, seq_num: SequenceNumber, packet: ArenaBox<Packet>) {
        let size = packet.size();

        let mut inflight_packets = self.inflight_packets.write().await;
        inflight_packets.insert(seq_num, packet);

        self.in_flight.fetch_add(size as u32, Ordering::AcqRel);
    }

    async fn remove_packets(&self, ack_number: SequenceNumber) -> usize {
        let mut size = 0;
        let mut inflight_packets = self.inflight_packets.write().await;

        inflight_packets
            .retain(|_, p| {
                !p.is_seq_less_equal(ack_number) || (false, size += p.size()).0
            });

        self.in_flight.fetch_sub(size as u32, Ordering::AcqRel);
        size
    }

    async fn remove_packet(&self, ack_number: SequenceNumber) -> usize {
        let mut inflight_packets = self.inflight_packets.write().await;

        let size = inflight_packets.remove(&ack_number)
                                   .map(|p| p.size())
                                   .unwrap_or(0);

        self.in_flight.fetch_sub(size as u32, Ordering::AcqRel);
        size
    }

    fn inflight_size(&self) -> usize {
        self.in_flight.load(Ordering::Acquire) as usize
    }

    fn utp_state(&self) -> UtpState {
        self.utp_state.load(Ordering::Acquire).into()
    }
    fn set_utp_state(&self, state: UtpState) {
        self.utp_state.store(state.into(), Ordering::Release)
    }
    fn recv_id(&self) -> ConnectionId {
        self.recv_id.load(Ordering::Relaxed).into()
    }
    fn set_recv_id(&self, recv_id: ConnectionId) {
        self.recv_id.store(recv_id.into(), Ordering::Release)
    }
    fn send_id(&self) -> ConnectionId {
        self.send_id.load(Ordering::Relaxed).into()
    }
    fn set_send_id(&self, send_id: ConnectionId) {
        self.send_id.store(send_id.into(), Ordering::Release)
    }
    fn ack_number(&self) -> SequenceNumber {
        self.ack_number.load(Ordering::Acquire).into()
    }
    fn set_ack_number(&self, ack_number: SequenceNumber) {
        self.ack_number.store(ack_number.into(), Ordering::Release)
    }
    fn seq_number(&self) -> SequenceNumber {
        self.seq_number.load(Ordering::Acquire).into()
    }
    fn set_seq_number(&self, seq_number: SequenceNumber) {
        self.seq_number.store(seq_number.into(), Ordering::Release)
    }
    /// Increment seq_number and returns its previous value
    fn increment_seq(&self) -> SequenceNumber {
        self.seq_number.fetch_add(1, Ordering::AcqRel).into()
    }
    fn remote_window(&self) -> u32 {
        self.remote_window.load(Ordering::Acquire)
    }
    fn set_remote_window(&self, remote_window: u32) {
        self.remote_window.store(remote_window, Ordering::Release)
    }
    fn cwnd(&self) -> u32 {
        self.cwnd.load(Ordering::Acquire)
    }
    fn set_cwnd(&self, cwnd: u32) {
        self.cwnd.store(cwnd, Ordering::Release)
    }
}

impl State {
    fn with_utp_state(utp_state: UtpState) -> State {
        State {
            utp_state: AtomicU8::new(utp_state.into()),
            ..Default::default()
        }
    }
}

impl Default for State {
    fn default() -> State {
        let (recv_id, send_id) = ConnectionId::make_ids();

        State {
            utp_state: AtomicU8::new(UtpState::None.into()),
            recv_id: AtomicU16::new(recv_id.into()),
            send_id: AtomicU16::new(send_id.into()),
            ack_number: AtomicU16::new(SequenceNumber::zero().into()),
            seq_number: AtomicU16::new(SequenceNumber::random().into()),
            remote_window: AtomicU32::new(INIT_CWND * MSS),
            cwnd: AtomicU32::new(INIT_CWND * MSS),
            in_flight: AtomicU32::new(0),
            inflight_packets: Default::default(),
        }
    }
}

const BUFFER_CAPACITY: usize = 1500;

pub struct UtpListener {
    v4: Arc<UdpSocket>,
    v6: Arc<UdpSocket>,
    /// The hashmap might be modified by different tasks so we wrap it in a RwLock
    streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>,
    packet_arena: Arc<SharedArena<Packet>>
}

enum IncomingEvent {
    V4((usize, SocketAddr)),
    V6((usize, SocketAddr)),
}

impl UtpListener {
    pub async fn new(port: u16) -> Result<Arc<UtpListener>> {
        use async_std::prelude::*;

        let v4 = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), port));
        let v6 = UdpSocket::bind(SocketAddrV6::new(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1), port, 0, 0));

        let (v4, v6) = v4.join(v6).await;
        let (v4, v6) = (v4?, v6?);

        let listener = Arc::new(UtpListener {
            v4: Arc::new(v4),
            v6: Arc::new(v6),
            streams: Default::default(),
            packet_arena: Default::default()
        });

        listener.clone().start();

        Ok(listener)
    }

    fn get_matching_socket(&self, sockaddr: &SocketAddr) -> Arc<UdpSocket> {
        if sockaddr.is_ipv4() {
            Arc::clone(&self.v4)
        } else {
            Arc::clone(&self.v6)
        }
    }

    pub async fn connect(&self, sockaddr: SocketAddr) -> Result<UtpStream> {
        let (on_connected, is_connected) = channel(1);

        let state = State::with_utp_state(UtpState::MustConnect);
        self.new_connection(sockaddr, state, on_connected).await;

        if let Ok(Some(stream)) = is_connected.recv().await {
            Ok(stream)
        } else {
            Err(Error::new(ErrorKind::TimedOut, "utp connect timed out").into())
        }
    }

    async fn new_connection(
        &self,
        sockaddr: SocketAddr,
        state: impl Into<Option<State>>,
        on_connected: impl Into<Option<Sender<Option<UtpStream>>>>,
    ) -> Sender<UtpEvent>
    {
        let socket = self.get_matching_socket(&sockaddr);

        let (sender, receiver) = channel(10);
        let state = Arc::new(state.into().unwrap_or_else(Default::default));

        let manager = UtpManager::new_with_state(
            socket,
            sockaddr,
            receiver,
            state,
            self.packet_arena.clone(),
            on_connected.into(),
        );

        {
            let mut streams = self.streams.write().await;
            streams.insert(sockaddr, sender.clone());
        }

        task::spawn(async move {
            // Put state of the manager in a separate alloc
            // https://www.reddit.com/r/rust/comments/gz5oe0/futures_and_segmented_stacks
            Box::pin(manager.start()).await
        });

        sender
    }

    pub fn start(self: Arc<Self>) {
        task::spawn(async move {
            Tick::new(self.streams.clone()).start();
            Box::pin(self.process_incoming()).await.unwrap()
        });
    }

    async fn poll(&self, buffer_v4: &mut [u8], buffer_v6: &mut [u8]) -> Result<IncomingEvent> {
        let v4 = self.v4.recv_from(buffer_v4);
        let v6 = self.v6.recv_from(buffer_v6);
        pin_mut!(v4); // Pin on the stack
        pin_mut!(v6); // Pin on the stack

        let fun = |cx: &mut Context<'_>| {
            match FutureExt::poll_unpin(&mut v4, cx).map_ok(IncomingEvent::V4) {
                v @ Poll::Ready(_) => return v,
                _ => {}
            }

            match FutureExt::poll_unpin(&mut v6, cx).map_ok(IncomingEvent::V6) {
                v @ Poll::Ready(_) => v,
                _ => Poll::Pending
            }
        };

        future::poll_fn(fun)
            .await
            .map_err(Into::into)
    }

    async fn poll_event(&self, buffer_v4: &mut [u8], buffer_v6: &mut [u8]) -> Result<IncomingEvent> {
        loop {
            match self.poll(buffer_v4, buffer_v6).await {
                Err(ref e) if e.should_continue() => {
                    // WouldBlock or TimedOut
                    continue
                }
                x => return x
            }
        }
    }

    async fn process_incoming(&self) -> Result<()> {
        use IncomingEvent::*;

        let mut buffer_v4 = [0; BUFFER_CAPACITY];
        let mut buffer_v6 = [0; BUFFER_CAPACITY];

        loop {
            let (buffer, addr) = match self.poll_event(&mut buffer_v4[..], &mut buffer_v6[..]).await? {
                V4((size, addr)) => {
                    (&buffer_v4[..size], addr)
                },
                V6((size, addr)) => {
                    (&buffer_v6[..size], addr)
                },
            };

            if buffer.len() < HEADER_SIZE || buffer.len() > PACKET_MAX_SIZE {
                continue;
            }

            let timestamp = Timestamp::now();

            let packet = self.packet_arena.alloc_with(|packet_uninit| {
                Packet::from_incoming_in_place(packet_uninit, buffer, timestamp)
            });

            {
                if let Some(addr) = self.streams.read().await.get(&addr) {
                    let incoming = UtpEvent::IncomingPacket { packet };

                    // self.streams is still borrowed at this point
                    // can add.send() blocks and so self.streams be deadlock ?
                    // A solution is to clone the addr, but it involves its drop overhead.
                    // Or use try_send when available and clone only if error
                    addr.send(incoming).await;
                    continue;
                }
            }

            if let Ok(PacketType::Syn) = packet.get_type() {
                let incoming = UtpEvent::IncomingPacket { packet };
                self.new_connection(addr, None, None)
                    .await
                    .send(incoming)
                    .await;
            }
        }
    }
}

use std::time::{Instant, Duration};

#[derive(Debug)]
struct UtpManager {
    socket: Arc<UdpSocket>,
    recv: Receiver<UtpEvent>,
    /// Do not await while locking the state
    /// The await could block and lead to a deadlock state
    state: Arc<State>,
    addr: SocketAddr,
    writer_user: Sender<WriterUserCommand>,
    writer: Sender<WriterCommand>,

    packet_arena: Arc<SharedArena<Packet>>,

    timeout: Instant,
    /// Number of consecutive times we've been timeout
    /// After 3 times we close the socket
    ntimeout: usize,

    on_connected: Option<Sender<Option<UtpStream>>>,

    ack_duplicate: u8,
    last_ack: SequenceNumber,

    delay_history: DelayHistory,

    tmp_packet_losts: Vec<SequenceNumber>,

    nlost: usize,

    slow_start: bool,

    next_cwnd_decrease: Instant,

    rtt: usize,
    rtt_var: usize,
    rto: usize
}

pub enum UtpEvent {
    IncomingPacket {
        packet: ArenaBox<Packet>
    },
    // IncomingBytes {
    //     buffer: Vec<u8>,
    //     timestamp: Timestamp
    // },
    Tick
}

impl Drop for UtpManager {
    fn drop(&mut self) {
        println!("DROP UTP MANAGER", );
        if let Some(on_connected) = self.on_connected.take() {
            task::spawn(async move {
                on_connected.send(None).await
            });
        };
    }
}

impl UtpManager {
    fn new_with_state(
        socket: Arc<UdpSocket>,
        addr: SocketAddr,
        recv: Receiver<UtpEvent>,
        state: Arc<State>,
        packet_arena: Arc<SharedArena<Packet>>,
        on_connected: Option<Sender<Option<UtpStream>>>,
    ) -> UtpManager {
        let (writer, writer_rcv) = channel(10);
        let (writer_user, writer_user_rcv) = channel(10);

        let writer_actor = UtpWriter::new(
            socket.clone(), addr, writer_user_rcv,
            writer_rcv, Arc::clone(&state), packet_arena.clone()
        );
        task::spawn(async move {
            writer_actor.start().await;
        });

        UtpManager {
            socket,
            addr,
            recv,
            writer,
            state,
            writer_user,
            packet_arena,
            timeout: Instant::now() + Duration::from_secs(3),
            ntimeout: 0,
            on_connected,
            last_ack: SequenceNumber::zero(),
            ack_duplicate: 0,
            delay_history: DelayHistory::new(),
            tmp_packet_losts: Vec::new(),
            nlost: 0,
            slow_start: true,
            next_cwnd_decrease: Instant::now(),
            rtt: 0,
            rtt_var: 0,
            rto: 0
        }
    }

    pub fn get_stream(&self) -> UtpStream {
        UtpStream {
            // reader_command: self.reader.clone(),
            // reader_result: self._reader_result_rcv.clone(),
            writer_user_command: self.writer_user.clone()
        }
    }

    async fn start(mut self) -> Result<()> {
        self.ensure_connected().await;
        while let Ok(incoming) = self.recv.recv().await {
            self.process_incoming(incoming).await.unwrap();
        }
        Ok(())
    }

    async fn ensure_connected(&self) {
        let state = self.state.utp_state();

        if state != UtpState::MustConnect {
            return;
        }

        self.writer.send(WriterCommand::SendPacket {
            packet_type: PacketType::Syn
        }).await;
    }

    async fn process_incoming(&mut self, event: UtpEvent) -> Result<()> {
        match event {
            UtpEvent::IncomingPacket { packet } => {
                match self.dispatch(packet).await {
                    Err(UtpError::PacketLost) => {
                        self.writer.send(WriterCommand::ResendPacket {
                            only_lost: true
                        }).await;
                    }
                    Ok(_) => {},
                    Err(e) => return Err(e)
                }
            }
            UtpEvent::Tick => {
                if Instant::now() > self.timeout {
                    self.on_timeout().await?;
                } else {
                    // println!("TICK RECEVEID BUT NOT TIMEOUT {:?} {:?}", self.timeout, self.rto);
                }
            }
        }
        Ok(())
    }

    async fn on_timeout(&mut self) -> Result<()> {
        use UtpState::*;

        let utp_state = self.state.utp_state();

        if (utp_state == SynSent && self.ntimeout >= 3)
            || self.ntimeout >= 7
        {
            return Err(Error::new(ErrorKind::TimedOut, "utp timed out").into());
        }

        match utp_state {
            SynSent => {
                self.writer.send(WriterCommand::SendPacket { packet_type: PacketType::Syn }).await;
            }
            Connected => {
                if self.state.inflight_size() > 0 {
                    self.writer.send(WriterCommand::ResendPacket { only_lost: false }).await;
                }
            }
            _ => {}
        }

        if self.state.inflight_size() > 0 {
            self.ntimeout += 1;
        } else {
            println!("NLOST {:?}", self.nlost);
            // println!("RTO {:?} RTT {:?} RTT_VAR {:?}", self.rto, self.rtt, self.rtt_var);
        }

        self.timeout = Instant::now() + Duration::from_millis(self.rto as u64);

        Ok(())
    }

    async fn dispatch(&mut self, packet: ArenaBox<Packet>) -> Result<()> {
        // println!("DISPATCH HEADER: {:?}", packet.header());

        let _delay = packet.received_at().delay(packet.get_timestamp());
        // self.delay = Delay::since(packet.get_timestamp());

        let utp_state = self.state.utp_state();

        self.ntimeout = 0;
        self.timeout = Instant::now() + Duration::from_millis(self.rto as u64);

        match (packet.get_type()?, utp_state) {
            (PacketType::Syn, UtpState::None) => {
                //println!("RECEIVED SYN {:?}", self.addr);
                let connection_id = packet.get_connection_id();

                self.state.set_utp_state(UtpState::Connected);
                self.state.set_recv_id(connection_id + 1);
                self.state.set_send_id(connection_id);
                self.state.set_seq_number(SequenceNumber::random());
                self.state.set_ack_number(packet.get_seq_number());
                self.state.set_remote_window(packet.get_window_size());

                self.writer.send(WriterCommand::SendPacket {
                    packet_type: PacketType::State
                }).await;
            }
            (PacketType::Syn, _) => {
            }
            (PacketType::State, UtpState::SynSent) => {
                // Related:
                // https://engineering.bittorrent.com/2015/08/27/drdos-udp-based-protocols-and-bittorrent/
                // https://www.usenix.org/system/files/conference/woot15/woot15-paper-adamsky.pdf
                // https://github.com/bittorrent/libutp/commit/13d33254262d46b638d35c4bc1a2f76cea885760

                self.state.set_utp_state(UtpState::Connected);
                self.state.set_ack_number(packet.get_seq_number() - 1);
                self.state.set_remote_window(packet.get_window_size());

                if let Some(notify) = self.on_connected.take() {
                    notify.send(Some(self.get_stream())).await;
                };

                self.state.remove_packets(packet.get_ack_number()).await;
            }
            (PacketType::State, UtpState::Connected) => {
                let remote_window = self.state.remote_window();

                if remote_window != packet.get_window_size() {
                    panic!("WINDOW SIZE CHANGED {:?}", packet.get_window_size());
                }

                self.handle_state(packet).await?;
            }
            (PacketType::State, _) => {
                // Wrong Packet
            }
            (PacketType::Data, _) => {
            }
            (PacketType::Fin, _) => {
            }
            (PacketType::Reset, _) => {
            }
        }

        Ok(())
    }


    async fn handle_state(&mut self, packet: ArenaBox<Packet>) -> Result<()> {
        let ack_number = packet.get_ack_number();
        let received_at = packet.received_at();

        // println!("ACK RECEIVED {:?} LAST_ACK {:?} DUP {:?} INFLIGHT {:?}",
        //          ack_number, self.last_ack, self.ack_duplicate, self.state.inflight_size());

        let in_flight = self.state.inflight_size();
        let mut bytes_newly_acked = 0;

        // let before = self.state.inflight_size();
        bytes_newly_acked += self.ack_packets(ack_number, received_at).await;
        // bytes_newly_acked += self.state.remove_packets(ack_number).await;
        //println!("PACKETS IN FLIGHT {:?}", self.inflight_packets.len());
        //        println!("PACKETS IN FLIGHT {:?}", self.inflight_packets.as_slices());

        // println!("BEFORE {:?} AFTER {:?}", before, self.state.inflight_size());

        let mut lost = false;

        if self.last_ack == ack_number {
            self.ack_duplicate = self.ack_duplicate.saturating_add(1);
            if self.ack_duplicate >= 3 {
                self.tmp_packet_losts.push(ack_number + 1);
                lost = true;
            }
        } else {
            self.ack_duplicate = 0;
            self.last_ack = ack_number;
        }

        if packet.has_extension() {
            //println!("HAS EXTENSIONS !", );
            for select_ack in packet.iter_sacks() {
                lost = select_ack.has_missing_ack() || lost;
                //println!("SACKS ACKED: {:?}", select_ack.nackeds());
                for ack_bit in select_ack {
                    match ack_bit {
                        SelectiveAckBit::Missing(seq_num) => {
                            self.tmp_packet_losts.push(seq_num);
                        }
                        SelectiveAckBit::Acked(seq_num) => {
                            bytes_newly_acked += self.ack_one_packet(seq_num, received_at).await;
//                            bytes_newly_acked += self.state.remove_packet(seq_num).await;
                        }
                    }
                }
            }
        }

        let delay = packet.get_timestamp_diff();
        if !delay.is_zero() {
            self.delay_history.add_delay(delay);
            if !lost {
                self.apply_congestion_control(bytes_newly_acked, in_flight);
            }
        }

        if lost {
            let now = Instant::now();

            if self.next_cwnd_decrease < now {
                let cwnd = self.state.cwnd();
                let cwnd = cwnd.min((cwnd / 2).max(MIN_CWND * MSS));
                self.state.set_cwnd(cwnd);
                self.next_cwnd_decrease = now + Duration::from_millis(100);
            }

            if self.slow_start {
                println!("STOP SLOW START AT {:?}", self.state.cwnd());
                self.slow_start = false;
            }

            self.nlost += self.tmp_packet_losts.len();

            self.mark_packets_as_lost().await;
            // println!("MISSING FROM SACK {:?}", self.lost_packets);
            return Err(UtpError::PacketLost);
        }

        self.rto = 500.max(self.rtt + self.rtt_var * 4);

        self.writer.send(WriterCommand::Acks).await;

        Ok(())
    }

    fn update_rtt(
        packet: &Packet,
        ack_received_at: Timestamp,
        mut rtt: usize,
        mut rtt_var: usize
    ) -> (usize, usize)
    {
        if !packet.resent {
            let rtt_packet = packet.get_timestamp().elapsed_millis(ack_received_at) as usize;

            if rtt == 0 {
                rtt = rtt_packet;
                rtt_var = rtt_packet / 2;
            } else {
                let delta: i64 = rtt as i64 - rtt_packet as i64;
                rtt_var = (rtt_var as i64 + (delta.abs() - rtt_var as i64) / 4) as usize;
                rtt = rtt - (rtt / 8) + (rtt_packet / 8);
            }
        }
        (rtt, rtt_var)
    }

    async fn ack_packets(&mut self, ack_number: SequenceNumber, ack_received_at: Timestamp) -> usize {
        // We need to make temporary vars to make the borrow checker happy
        let (mut rtt, mut rtt_var) = (self.rtt, self.rtt_var);

        let mut size = 0;
        let mut inflight_packets = self.state.inflight_packets.write().await;

        inflight_packets
            .retain(|_, p| {
                !p.is_seq_less_equal(ack_number) || {
                    let (r, rv) = Self::update_rtt(p, ack_received_at, rtt, rtt_var);
                    rtt = r;
                    rtt_var = rv;
                    size += p.size();
                    false
                }
            });

        self.rtt = rtt;
        self.rtt_var = rtt_var;

        self.state.in_flight.fetch_sub(size as u32, Ordering::AcqRel);
        size
    }

    async fn ack_one_packet(&mut self, ack_number: SequenceNumber, ack_received_at: Timestamp) -> usize {
        let mut inflight_packets = self.state.inflight_packets.write().await;
        let mut size = 0;

        if let Some(ref packet) = inflight_packets.remove(&ack_number) {
            let (r, rv) = Self::update_rtt(packet, ack_received_at, self.rtt, self.rtt_var);
            self.rtt = r;
            self.rtt_var = rv;
            size = packet.size();
            self.state.in_flight.fetch_sub(size as u32, Ordering::AcqRel);
        }

        size
    }

    async fn mark_packets_as_lost(&mut self) {
        let mut inflight_packets = self.state.inflight_packets.write().await;
        for seq in &self.tmp_packet_losts {
            if let Some(packet) = inflight_packets.get_mut(seq) {
                packet.lost = true;
            };
        }
        self.tmp_packet_losts.clear();
    }

    fn apply_congestion_control(&mut self, bytes_newly_acked: usize, in_flight: usize) {
        let lowest_relative = self.delay_history.lowest_relative();

        let cwnd = self.state.cwnd() as usize;
        // let before = cwnd;

        let cwnd_reached = in_flight + bytes_newly_acked + self.packet_size() > cwnd;

        if cwnd_reached {
            let cwnd = if self.slow_start {
                (cwnd + bytes_newly_acked) as u32
            } else {
                let window_factor = I48F16::from_num(bytes_newly_acked as i64) / in_flight as i64;
                let delay_factor = I48F16::from_num(TARGET - lowest_relative.as_i64()) / TARGET;

                let gain = (window_factor * delay_factor) * 3000;

                (cwnd as i32 + gain.to_num::<i32>()).max(0) as u32
            };
            let cwnd = self.state.remote_window().min(cwnd);
            self.state.set_cwnd(cwnd);

            //println!("CWND: {:?} BEFORE={}", cwnd, before);

            //println!("!! CWND CHANGED !! {:?} WIN_FACT {:?} DELAY_FACT {:?} GAIN {:?}",
            // cwnd, window_factor, delay_factor, gain);
        }
    }

    fn packet_size(&self) -> usize {
        let is_ipv4 = self.addr.is_ipv4();

        // TODO: Change this when MTU discovery is implemented
        if is_ipv4 {
            UDP_IPV4_MTU - HEADER_SIZE
        } else {
            UDP_IPV6_MTU - HEADER_SIZE
        }
    }
}

#[derive(Debug)]
struct UtpWriter {
    socket: Arc<UdpSocket>,
    addr: SocketAddr,
    user_command: Receiver<WriterUserCommand>,
    command: Receiver<WriterCommand>,
    state: Arc<State>,
    packet_arena: Arc<SharedArena<Packet>>
}

impl Drop for UtpWriter {
    fn drop(&mut self) {
        println!("UtpWriter Dropped", );
    }
}

#[derive(Debug)]
enum WriterCommand {
    WriteData {
        data: Vec<u8>,
    },
    SendPacket {
        packet_type: PacketType
    },
    ResendPacket {
        only_lost: bool
    },
    Acks,
}

struct WriterUserCommand {
    data: Vec<u8>
}

impl UtpWriter {
    pub fn new(
        socket: Arc<UdpSocket>,
        addr: SocketAddr,
        user_command: Receiver<WriterUserCommand>,
        command: Receiver<WriterCommand>,
        state: Arc<State>,
        packet_arena: Arc<SharedArena<Packet>>
    ) -> UtpWriter {
        UtpWriter { socket, addr, user_command, command, state, packet_arena }
    }

    async fn poll(&self) -> Result<WriterCommand> {
        let user_cmd = self.user_command.recv();
        let cmd = self.command.recv();
        pin_mut!(user_cmd); // Pin on the stack
        pin_mut!(cmd); // Pin on the stack

        let fun = |cx: &mut Context<'_>| {
            match FutureExt::poll_unpin(&mut cmd, cx) {
                v @ Poll::Ready(_) => return v,
                _ => {}
            }

            match FutureExt::poll_unpin(&mut user_cmd, cx).map(|cmd| {
                cmd.map(|c| WriterCommand::WriteData { data: c.data })
            }) {
                v @ Poll::Ready(_) => v,
                _ => Poll::Pending
            }
        };

        future::poll_fn(fun).await.map_err(UtpError::RecvError)
    }

    pub async fn start(mut self) {
        while let Ok(cmd) = self.poll().await {
            self.handle_cmd(cmd).await.unwrap();
        }
    }

    async fn handle_cmd(&mut self, cmd: WriterCommand) -> Result<()> {
        use WriterCommand::*;

        match cmd {
            WriteData { ref data } => {
                // loop {
                    self.send(data).await.unwrap();
                    println!("== SEND DATA {:#?}", self.packet_arena);
                    //self.packet_arena.resize(1600);
                // }
            },
            SendPacket { packet_type } => {
                if packet_type == PacketType::Syn {
                    self.send_syn().await.unwrap();
                } else {
                    let packet = self.packet_arena.alloc(Packet::new_type(packet_type));
                    self.send_packet(packet).await.unwrap();
                }
            }
            ResendPacket { only_lost } => {
                self.resend_packets(only_lost).await.unwrap();
            }
            Acks => {
                return Ok(());
            }
        }
        Ok(())
    }

    async fn handle_manager_cmd(&mut self, cmd: WriterCommand) -> Result<()> {
        use WriterCommand::*;

        match cmd {
            SendPacket { packet_type } => {
                if packet_type == PacketType::Syn {
                    self.send_syn().await.unwrap();
                } else {
                    let packet = self.packet_arena.alloc(Packet::new_type(packet_type));
                    self.send_packet(packet).await.unwrap();
                }
            }
            ResendPacket { only_lost } => {
                self.resend_packets(only_lost).await.unwrap();
            }
            Acks => {
                return Ok(());
            }
            WriteData { .. } => {
                unreachable!();
            },
        }
        Ok(())
    }

    async fn resend_packets(&mut self, only_lost: bool) -> Result<()> {
        let mut inflight_packets = self.state.inflight_packets.write().await;

        // println!("RESENDING ONLY_LOST={:?} INFLIGHT_LEN={:?} CWND={:?}", only_lost, inflight_packets.len(), self.state.cwnd());

        // let mut resent = 0;

        for packet in inflight_packets.values_mut() {
            if !only_lost || packet.lost {
                packet.set_ack_number(self.state.ack_number());
                packet.update_timestamp();

                // println!("== RESENDING {:?}", &**packet);
//                println!("== RESENDING {:?} CWND={:?} POS {:?} {:?}", start, self.cwnd, start, &**packet);

                // resent += 1;
                self.socket.send_to(packet.as_bytes(), self.addr).await?;

                packet.resent = true;
                packet.lost = false;
            }
        }

        // println!("RESENT: {:?}", resent);
        Ok(())
    }

    async fn send(&mut self, data: &[u8]) -> Result<()> {
        let packet_size = self.packet_size();

        let packets = data.chunks(packet_size);

        for packet in packets {
            let packet = self.packet_arena.alloc_with(|packet_uninit| {
                Packet::new_in_place(packet_uninit, packet)
            });

            // let data = arena.alloc_with(|uninit| {
            //     Packet::new_with(uninit, source)
            // })

            self.ensure_window_is_large_enough(packet.size()).await?;
            self.send_packet(packet).await?;
        }

        eprintln!("DOOOONE", );

        Ok(())
    }

    fn packet_size(&self) -> usize {
        let is_ipv4 = self.addr.is_ipv4();

        // TODO: Change this when MTU discovery is implemented
        if is_ipv4 {
            UDP_IPV4_MTU - HEADER_SIZE
        } else {
            UDP_IPV6_MTU - HEADER_SIZE
        }
    }

    async fn ensure_window_is_large_enough(&mut self, packet_size: usize) -> Result<()> {
        while packet_size + self.state.inflight_size() > self.state.cwnd() as usize {
            // println!("BLOCKED BY CWND {:?} {:?} {:?}", packet_size, self.state.inflight_size(), self.state.cwnd());

            // Too many packets in flight, we wait for acked or lost packets
            match self.command.recv().await {
                Ok(cmd) => self.handle_manager_cmd(cmd).await?,
                _ => return Err(UtpError::MustClose)
            }
        }
        Ok(())
    }

    async fn send_packet(&mut self, mut packet: ArenaBox<Packet>) -> Result<()> {
        let ack_number = self.state.ack_number();
        let seq_number = self.state.increment_seq();
        let send_id = self.state.send_id();

        packet.set_ack_number(ack_number);
        packet.set_packet_seq_number(seq_number);
        packet.set_connection_id(send_id);
        packet.set_window_size(1_048_576);

        //println!("SENDING NEW PACKET ! {:?}", packet);

        packet.update_timestamp();

        self.socket.send_to(packet.as_bytes(), self.addr).await?;

        self.state.add_packet_inflight(seq_number, packet).await;

        Ok(())
    }

    async fn send_syn(&mut self) -> Result<()> {
        let seq_number = self.state.increment_seq();
        let recv_id = self.state.recv_id();

        let mut packet = Packet::syn();
        packet.set_connection_id(recv_id);
        packet.set_packet_seq_number(seq_number);
        packet.set_window_size(1_048_576);

        //println!("SENDING {:#?}", packet);

        packet.update_timestamp();
        self.socket.send_to(packet.as_bytes(), self.addr).await?;

        self.state.set_utp_state(UtpState::SynSent);

        let packet = self.packet_arena.alloc(packet);
        self.state.add_packet_inflight(seq_number, packet).await;

        Ok(())
    }
}

#[derive(Debug)]
pub struct UtpStream {
    // reader_command: Sender<ReaderCommand>,
    // reader_result: Receiver<ReaderResult>,
    writer_user_command: Sender<WriterUserCommand>,
}

impl UtpStream {
    pub async fn read(&self, _data: &mut [u8]) {
        // self.reader_command.send(ReaderCommand {
        //     length: data.len()
        // }).await;

        // self.reader_result.recv().await;
    }

    pub async fn write(&self, data: &[u8]) {
        let data = Vec::from_slice(data);
        self.writer_user_command.send(WriterUserCommand { data }).await;
    }
}
