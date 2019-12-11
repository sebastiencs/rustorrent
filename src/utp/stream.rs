
use async_std::sync::{Arc, channel, Sender, Receiver, RwLock};
use async_std::task;
use async_std::io::{ErrorKind, Error};
use async_std::net::{SocketAddr, UdpSocket, SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr};
use futures::task::{Context, Poll};
use futures::{future, pin_mut};
use futures::future::{Fuse, FutureExt};
use hashbrown::HashMap;
use fixed::types::I48F16;

use std::collections::VecDeque;
use std::cell::RefCell;

use crate::udp_ext::WithTimeout;
use crate::utils::Map;

use super::{
    UtpError, Result, SequenceNumber, Packet, PacketRef,
    Timestamp, PacketType, ConnectionId, HEADER_SIZE,
    SelectiveAckBit, SelectiveAck, DelayHistory, Delay, RelativeDelay,
    UDP_IPV4_MTU, UDP_IPV6_MTU
};
use super::socket::{
    INIT_CWND, MSS,
    State as UtpState,
    TARGET, MIN_CWND
};
use super::tick::Tick;
use crate::utils::FromSlice;

use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, Ordering};
use crate::cache_line::CacheAligned;

#[derive(Debug)]
struct AtomicState {
    utp_state: CacheAligned<AtomicU8>,
    recv_id: CacheAligned<AtomicU16>,
    send_id: CacheAligned<AtomicU16>,
    ack_number: CacheAligned<AtomicU16>,
    seq_number: CacheAligned<AtomicU16>,
    remote_window: CacheAligned<AtomicU32>,
    cwnd: CacheAligned<AtomicU32>,
    in_flight: CacheAligned<AtomicU32>,
}

impl Default for AtomicState {
    fn default() -> AtomicState {
        let (recv_id, send_id) = ConnectionId::make_ids();

        AtomicState {
            utp_state: CacheAligned::new(AtomicU8::new(UtpState::None.into())),
            recv_id: CacheAligned::new(AtomicU16::new(recv_id.into())),
            send_id: CacheAligned::new(AtomicU16::new(send_id.into())),
            ack_number: CacheAligned::new(AtomicU16::new(SequenceNumber::zero().into())),
            seq_number: CacheAligned::new(AtomicU16::new(SequenceNumber::random().into())),
            remote_window: CacheAligned::new(AtomicU32::new(INIT_CWND * MSS)),
            cwnd: CacheAligned::new(AtomicU32::new(INIT_CWND * MSS)),
            in_flight: CacheAligned::new(AtomicU32::new(0))
        }
    }
}

#[derive(Debug)]
struct State {
    atomic: AtomicState,

    /// Packets sent but we didn't receive an ack for them
    inflight_packets: RwLock<Map<SequenceNumber, Packet>>,
}

impl State {
    async fn add_packet_inflight(&self, seq_num: SequenceNumber, packet: Packet) {
        let size = packet.size();

        let mut inflight_packets = self.inflight_packets.write().await;
        inflight_packets.insert(seq_num, packet);

        self.atomic.in_flight.fetch_add(size as u32, Ordering::AcqRel);
    }

    async fn remove_packets(&self, ack_number: SequenceNumber) -> usize {
        let mut size = 0;
        let mut inflight_packets = self.inflight_packets.write().await;

        inflight_packets
            .retain(|_, p| {
                !p.is_seq_less_equal(ack_number) || (false, size += p.size()).0
            });

        self.atomic.in_flight.fetch_sub(size as u32, Ordering::AcqRel);
        size
    }

    async fn remove_packet(&self, ack_number: SequenceNumber) -> usize {
        let mut inflight_packets = self.inflight_packets.write().await;

        let size = inflight_packets.remove(&ack_number)
                                   .map(|p| p.size())
                                   .unwrap_or(0);

        self.atomic.in_flight.fetch_sub(size as u32, Ordering::AcqRel);
        size
    }

    fn inflight_size(&self) -> usize {
        self.atomic.in_flight.load(Ordering::Acquire) as usize
    }

    fn utp_state(&self) -> UtpState {
        self.atomic.utp_state.load(Ordering::Acquire).into()
    }
    fn set_utp_state(&self, state: UtpState) {
        self.atomic.utp_state.store(state.into(), Ordering::Release)
    }
    fn recv_id(&self) -> ConnectionId {
        self.atomic.recv_id.load(Ordering::Relaxed).into()
    }
    fn set_recv_id(&self, recv_id: ConnectionId) {
        self.atomic.recv_id.store(recv_id.into(), Ordering::Release)
    }
    fn send_id(&self) -> ConnectionId {
        self.atomic.send_id.load(Ordering::Relaxed).into()
    }
    fn set_send_id(&self, send_id: ConnectionId) {
        self.atomic.send_id.store(send_id.into(), Ordering::Release)
    }
    fn ack_number(&self) -> SequenceNumber {
        self.atomic.ack_number.load(Ordering::Acquire).into()
    }
    fn set_ack_number(&self, ack_number: SequenceNumber) {
        self.atomic.ack_number.store(ack_number.into(), Ordering::Release)
    }
    fn seq_number(&self) -> SequenceNumber {
        self.atomic.seq_number.load(Ordering::Acquire).into()
    }
    fn set_seq_number(&self, seq_number: SequenceNumber) {
        self.atomic.seq_number.store(seq_number.into(), Ordering::Release)
    }
    /// Increment seq_number and returns its previous value
    fn increment_seq(&self) -> SequenceNumber {
        self.atomic.seq_number.fetch_add(1, Ordering::AcqRel).into()
    }
    fn remote_window(&self) -> u32 {
        self.atomic.remote_window.load(Ordering::Acquire)
    }
    fn set_remote_window(&self, remote_window: u32) {
        self.atomic.remote_window.store(remote_window, Ordering::Release)
    }
    fn cwnd(&self) -> u32 {
        self.atomic.cwnd.load(Ordering::Acquire)
    }
    fn set_cwnd(&self, cwnd: u32) {
        self.atomic.cwnd.store(cwnd, Ordering::Release)
    }
}

impl State {
    fn with_utp_state(utp_state: UtpState) -> State {
        State {
            atomic: AtomicState {
                utp_state: CacheAligned::new(AtomicU8::new(utp_state.into())),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

impl Default for State {
    fn default() -> State {
        State {
            atomic: Default::default(),
            // delay: Delay::default(),
            // current_delays: VecDeque::with_capacity(16),
            // last_rollover: Instant::now(),
            // congestion_timeout: Duration::from_secs(1),
            // flight_size: 0,
            // srtt: 0,
            // rttvar: 0,
            inflight_packets: Default::default(),
            // delay_history: DelayHistory::new(),
            // ack_duplicate: 0,
            // last_ack: SequenceNumber::zero(),
            // lost_packets: VecDeque::with_capacity(100),
            // nlost: 0,
        }
    }
}

const BUFFER_CAPACITY: usize = 1500;

pub struct UtpListener {
    v4: Arc<UdpSocket>,
    v6: Arc<UdpSocket>,
    /// The hashmap might be modified by different tasks so we wrap it in a RwLock
    streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>,
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
        let socket = self.get_matching_socket(&sockaddr);

        let (on_connected, is_connected) = channel(1);

        let state = State::with_utp_state(UtpState::MustConnect);
        let state = Arc::new(state);

        let (sender, receiver) = channel(10);
        let mut manager = UtpManager::new_with_state(socket, sockaddr, receiver, state);
        manager.set_on_connected(on_connected);

        {
            let mut streams = self.streams.write().await;
            streams.insert(sockaddr, sender.clone());
        }

        let stream = manager.get_stream();

        task::spawn(async move { manager.start().await });

        if let Some(true) = is_connected.recv().await {
            Ok(stream)
        } else {
            Err(Error::new(ErrorKind::TimedOut, "utp connect timed out").into())
        }
    }

    async fn new_connection(&self, sockaddr: SocketAddr) -> Sender<UtpEvent> {
        //println!("NEW CONNECTION {:?}", sockaddr);
        let socket = if sockaddr.is_ipv4() {
            Arc::clone(&self.v4)
        } else {
            Arc::clone(&self.v6)
        };

        let (sender, receiver) = channel(10);
        let manager = UtpManager::new(socket, sockaddr, receiver);

        {
            let mut streams = self.streams.write().await;
            streams.insert(sockaddr, sender.clone());
        }

        task::spawn(async move {
            manager.start().await
        });

        sender
    }

    pub fn start(self: Arc<Self>) {
        task::spawn(async move {
            Tick::new(self.streams.clone()).start();
            self.process_incoming().await.unwrap()
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
                    (Vec::from_slice(&buffer_v4[..size]), addr)
                },
                V6((size, addr)) => {
                    (Vec::from_slice(&buffer_v6[..size]), addr)
                },
            };

            let timestamp = Timestamp::now();

            //println!("INCOMING {:?} {:?}", buffer.len(), addr);

            if buffer.len() < HEADER_SIZE {
                continue;
            }


            {
                if let Some(addr) = self.streams.read().await.get(&addr) {
                    let incoming = UtpEvent::IncomingBytes { buffer, timestamp };

                    // self.streams is still borrowed at this point
                    // can add.send() blocks and so self.streams be deadlock ?
                    // A solution is to clone the addr, but it involves its drop overhead.
                    // Or use try_send when available and clone only if error
                    addr.send(incoming).await;
                    continue;
                }
            }

            let packet = PacketRef::ref_from_incoming(&buffer, timestamp);

            if let Ok(PacketType::Syn) = packet.get_type() {
                let incoming = UtpEvent::IncomingBytes { buffer, timestamp };
                self.new_connection(addr)
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

    timeout: Instant,
    /// Number of consecutive times we've been timeout
    /// After 3 times we close the socket
    ntimeout: usize,

    on_connected: Option<Sender<bool>>,

    ack_duplicate: u8,
    last_ack: SequenceNumber,

    delay_history: DelayHistory,

    tmp_packet_losts: Vec<SequenceNumber>,

    nlost: usize,

    slow_start: bool,

    next_cwnd_decrease: Instant,
}

pub enum UtpEvent {
    IncomingBytes {
        buffer: Vec<u8>,
        timestamp: Timestamp
    },
    Tick
}

impl Drop for UtpManager {
    fn drop(&mut self) {
        println!("DROP UTP MANAGER", );
        if let Some(on_connected) = self.on_connected.take() {
            task::spawn(async move {
                on_connected.send(false).await
            });
        };
    }
}

impl UtpManager {
    fn new(
        socket: Arc<UdpSocket>,
        addr: SocketAddr,
        recv: Receiver<UtpEvent>
    ) -> UtpManager {
        Self::new_with_state(socket, addr, recv, Default::default())
    }

    fn new_with_state(
        socket: Arc<UdpSocket>,
        addr: SocketAddr,
        recv: Receiver<UtpEvent>,
        state: Arc<State>,
    ) -> UtpManager {
        let (writer, writer_rcv) = channel(10);
        let (writer_user, writer_user_rcv) = channel(10);

        let writer_actor = UtpWriter::new(socket.clone(), addr, writer_user_rcv, writer_rcv, Arc::clone(&state));
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
            timeout: Instant::now() + Duration::from_secs(3),
            ntimeout: 0,
            on_connected: None,
            last_ack: SequenceNumber::zero(),
            ack_duplicate: 0,
            delay_history: DelayHistory::new(),
            tmp_packet_losts: Vec::new(),
            nlost: 0,
            slow_start: true,
            next_cwnd_decrease: Instant::now()
        }
    }

    pub fn set_on_connected(&mut self, on_connected: Sender<bool>) {
        self.on_connected = Some(on_connected);
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
        while let Some(incoming) = self.recv.recv().await {
            self.process_incoming(incoming).await?;
        }
        Ok(())
    }

    async fn ensure_connected(&self) {
        let state = self.state.utp_state();

        if state != UtpState::MustConnect {
            return;
        }

        self.writer.send(WriterCommand::SendPacket { packet_type: PacketType::Syn }).await;
    }

    async fn process_incoming(&mut self, event: UtpEvent) -> Result<()> {
        match event {
            UtpEvent::IncomingBytes { buffer, timestamp } => {
                let packet = PacketRef::ref_from_incoming(&buffer, timestamp);
                match self.dispatch(packet).await {
                    Err(UtpError::PacketLost) => {
                        self.writer.send(WriterCommand::ResendPacket { only_lost: true }).await;
                    }
                    Ok(_) => {},
                    Err(e) => return Err(e)
                }
            }
            UtpEvent::Tick => {
                if Instant::now() > self.timeout {
                    self.on_timeout().await?;
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
            return Err(Error::new(ErrorKind::TimedOut, "utp connect timed out").into());
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
        }

        self.timeout = Instant::now() + Duration::from_secs(1);

        Ok(())
    }

    async fn dispatch(&mut self, packet: PacketRef<'_>) -> Result<()> {
        //println!("DISPATCH HEADER: {:?}", packet.header());

        let delay = packet.received_at.delay(packet.get_timestamp());
        // self.delay = Delay::since(packet.get_timestamp());

        let utp_state = self.state.utp_state();

        self.ntimeout = 0;
        self.timeout = Instant::now() + Duration::from_secs(1);

        match (packet.get_type()?, utp_state) {
            (PacketType::Syn, UtpState::None) => {
                //println!("RECEIVED SYN {:?}", self.addr);
                // Set self.remote
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
                    notify.send(true).await;
                };

                self.state.remove_packets(packet.get_ack_number()).await;
            }
            (PacketType::State, UtpState::Connected) => {
                let remote_window = self.state.remote_window();

                if remote_window != packet.get_window_size() {
                    panic!("WINDOW SIZE CHANGED {:?}", packet.get_window_size());
                }

                self.handle_state(packet).await?;
                // let current_delay = packet.get_timestamp_diff();
                // let base_delay = std::cmp::min();
                // current_delay = acknowledgement.delay
                // base_delay = min(base_delay, current_delay)
                // queuing_delay = current_delay - base_delay
                // off_target = (TARGET - queuing_delay) / TARGET
                // cwnd += GAIN * off_target * bytes_newly_acked * MSS / cwnd
                // Ack received
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


    async fn handle_state(&mut self, packet: PacketRef<'_>) -> Result<()> {
        let ack_number = packet.get_ack_number();

        // println!("ACK RECEIVED {:?} LAST_ACK {:?} DUP {:?} INFLIGHT {:?}",
        //          ack_number, self.last_ack, self.ack_duplicate, self.state.inflight_size());

        let in_flight = self.state.inflight_size();
        let mut bytes_newly_acked = 0;

        let before = self.state.inflight_size();
        bytes_newly_acked += self.state.remove_packets(ack_number).await;
        // {
        //     let mut inflight_packets = self.state.inflight_packets.write().await;
        //     inflight_packets
        //         .retain(|p| {
        //             !p.is_seq_less_equal(ack_number) || (false, bytes_newly_acked += p.size()).0
        //         });
        // }
        //println!("PACKETS IN FLIGHT {:?}", self.inflight_packets.len());
        //        println!("PACKETS IN FLIGHT {:?}", self.inflight_packets.as_slices());

        // println!("BEFORE {:?} AFTER {:?}", before, self.state.inflight_size());

        let mut lost = false;

        if self.last_ack == ack_number {
            self.ack_duplicate = self.ack_duplicate.saturating_add(1);
            if self.ack_duplicate >= 3 {
                self.tmp_packet_losts.push(ack_number + 1);
                // self.mark_packet_as_lost(ack_number + 1).await;
                // self.lost_packets.push_back(ack_number + 1);
                lost = true;
                //return Err(UtpError::PacketLost);
            }
        } else {
            self.ack_duplicate = 0;
            self.last_ack = ack_number;
        }

        if packet.has_extension() {
            //println!("HAS EXTENSIONS !", );
            for select_ack in packet.iter_extensions() {
                lost = select_ack.has_missing_ack() || lost;
                //println!("SACKS ACKED: {:?}", select_ack.nackeds());
                for ack_bit in select_ack {
                    match ack_bit {
                        SelectiveAckBit::Missing(seq_num) => {
                            self.tmp_packet_losts.push(seq_num);
                            //self.mark_packet_as_lost(seq_num).await;
                        }
                        SelectiveAckBit::Acked(seq_num) => {
                            //println!("SACKED {:?}", seq_num);
                            bytes_newly_acked += self.state.remove_packet(seq_num).await;
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

        self.writer.send(WriterCommand::Acks).await;

        Ok(())
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
        let before = cwnd;

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

// #[derive(Debug)]
// struct UtpReader {
//     rcv: Receiver<ReaderCommand>,
//     send: Sender<ReaderResult>,
//     /// Do not await while locking the state
//     /// The await could block and lead to a deadlock state
//     state: Arc<RwLock<State>>,
// }

// impl UtpReader {
//     fn new(
//         rcv: Receiver<ReaderCommand>,
//         send: Sender<ReaderResult>,
//         state: Arc<RwLock<State>>,
//     ) -> UtpReader {
//         UtpReader { rcv, send, state }
//     }

//     async fn start(self) {

//     }
// }

// impl Drop for UtpReader {
//     fn drop(&mut self) {
//         println!("UtpReader Dropped", );
//     }
// }

// #[derive(Debug)]
// struct ReaderCommand {
//     length: usize
// }

// #[derive(Debug)]
// struct ReaderResult {
//     data: Vec<u8>
// }

#[derive(Debug)]
struct UtpWriter {
    socket: Arc<UdpSocket>,
    addr: SocketAddr,
    user_command: Receiver<WriterUserCommand>,
    command: Receiver<WriterCommand>,
    state: Arc<State>
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
    // Close,
    Acks,
}

struct WriterUserCommand {
    data: Vec<u8>
}

// #[derive(Debug)]
// struct WriterResult {
//     data: Vec<u8>
// }

impl UtpWriter {
    pub fn new(
        socket: Arc<UdpSocket>,
        addr: SocketAddr,
        user_command: Receiver<WriterUserCommand>,
        command: Receiver<WriterCommand>,
        state: Arc<State>,
    ) -> UtpWriter {
        UtpWriter { socket, addr, user_command, command, state }
    }

    async fn poll(&self) -> Option<WriterCommand> {
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

        future::poll_fn(fun).await
    }

    pub async fn start(mut self) {
//        while let Some(cmd) = self.command.recv().await {
        while let Some(cmd) = self.poll().await {
            self.handle_cmd(cmd).await.unwrap();
        }
    }

    async fn handle_cmd(&mut self, cmd: WriterCommand) -> Result<()> {
        use WriterCommand::*;

        match cmd {
            WriteData { ref data } => {
                self.send(data).await.unwrap()
            },
            SendPacket { packet_type } => {
                self.send_packet(Packet::new_type(packet_type)).await.unwrap();
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
                    self.send_packet(Packet::new_type(packet_type)).await.unwrap();
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

        let mut resent = 0;

        for packet in inflight_packets.values_mut() {
            if !only_lost || packet.lost {
                // let packet_size = packet.size();

                // if !packet.resent {
                packet.set_ack_number(self.state.ack_number());
                packet.update_timestamp();

                // println!("== RESENDING {:?}", &**packet);
//                println!("== RESENDING {:?} CWND={:?} POS {:?} {:?}", start, self.cwnd, start, &**packet);

                resent += 1;
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
        let packets = data.chunks(packet_size).map(Packet::new);

        let mut size = 0;

        //let mut packet_total = 0;

        for packet in packets {
            //println!("LOOP SEND", );
            //packet_total += 1;
            size += packet.payload_len();

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
            //println!("BLOCKED BY CWND {:?} {:?} {:?}", packet_size, self.state.inflight_size(), self.state.cwnd());
            match self.command.recv().await {
                Some(cmd) => self.handle_manager_cmd(cmd).await?,
                _ => return Err(UtpError::MustClose)
            }
            // self.receive_and_handle_lost_packets().await?;
        }
        Ok(())
    }

    // /// Returns the number of bytes currently in flight (sent but not acked)
    // async fn inflight_size(&self) -> usize {
    //     self.state.inflight_packets.read().await.iter().map(Packet::size).sum()
    // }

    async fn send_packet(&mut self, mut packet: Packet) -> Result<()> {
        let ack_number = self.state.ack_number();
        let seq_number = self.state.increment_seq();
        let send_id = self.state.send_id();

        packet.set_ack_number(ack_number);
        packet.set_packet_seq_number(seq_number);
        packet.set_connection_id(send_id);
        packet.set_window_size(1_048_576);

        //println!("SENDING NEW PACKET ! {:?}", packet);

        // println!("SENDING {:?}", &*packet);
        packet.update_timestamp();

        self.socket.send_to(packet.as_bytes(), self.addr).await?;

        self.state.add_packet_inflight(seq_number, packet).await;
        // {
        //     let mut inflight_packets = self.state.inflight_packets.write().await;
        //     inflight_packets.push_back(packet);
        // }

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
        self.state.add_packet_inflight(seq_number, packet).await;
        // {
        //     let mut inflight_packets = self.state.inflight_packets.write().await;
        //     inflight_packets.push_back(packet);
        // }

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
    pub async fn read(&self, data: &mut [u8]) {
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
