
use async_std::sync::{Arc, channel, Sender, Receiver, RwLock};
use async_std::task;
use async_std::io::{ErrorKind, Error};
use async_std::net::{SocketAddr, UdpSocket, SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr};
use futures::task::{Context, Poll};
use futures::{future, pin_mut};
use futures::future::{Fuse, FutureExt};
use hashbrown::HashMap;

use std::collections::VecDeque;
use std::cell::RefCell;

use crate::udp_ext::WithTimeout;

use super::{
    UtpError, Result, SequenceNumber, Packet, PacketRef,
    Timestamp, PacketType, ConnectionId, HEADER_SIZE,
};
use super::socket::{
    INIT_CWND, MSS,
    State as UtpState
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
        }
    }
}

#[derive(Debug)]
struct State {
    atomic: AtomicState,

    /// Packets sent but we didn't receive an ack for them
    inflight_packets: RwLock<VecDeque<Packet>>,
}

impl State {
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
            inflight_packets: RwLock::new(VecDeque::with_capacity(64)),
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

        let v4 = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port));
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

        task::spawn(async move {
            manager.start().await
        });

        if let Some(true) = is_connected.recv().await {
            Ok(stream)
        } else {
            Err(Error::new(ErrorKind::TimedOut, "utp connect timed out").into())
        }
    }

    async fn new_connection(&self, sockaddr: SocketAddr) -> Sender<UtpEvent> {
        println!("NEW CONNECTION {:?}", sockaddr);
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

            println!("INCOMING {:?} {:?}", buffer.len(), addr);

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
    writer: Sender<WriterCommand>,

    timeout: Instant,
    /// Number of consecutive times we've been timeout
    /// After 3 times we close the socket
    ntimeout: usize,

    on_connected: Option<Sender<bool>>,
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
        let on_connected = self.on_connected.take();
        let writer = self.writer.clone();
        task::spawn(async move {
            if let Some(on_connected) = on_connected {
                on_connected.send(false).await
            };
            writer.send(WriterCommand::Close).await
        });
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

        let writer_actor = UtpWriter::new(socket.clone(), addr, writer_rcv, Arc::clone(&state));
        task::spawn(async move {
            writer_actor.start().await;
        });

        UtpManager {
            socket,
            addr,
            recv,
            writer,
            state,
            timeout: Instant::now() + Duration::from_secs(3),
            ntimeout: 0,
            on_connected: None
        }
    }

    pub fn set_on_connected(&mut self, on_connected: Sender<bool>) {
        self.on_connected = Some(on_connected);
    }

    pub fn get_stream(&self) -> UtpStream {
        UtpStream {
            // reader_command: self.reader.clone(),
            // reader_result: self._reader_result_rcv.clone(),
            writer_command: self.writer.clone()
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

        self.writer.send(WriterCommand::SendSyn).await;
    }

    async fn process_incoming(&mut self, event: UtpEvent) -> Result<()> {
        match event {
            UtpEvent::IncomingBytes { buffer, timestamp } => {
                let packet = PacketRef::ref_from_incoming(&buffer, timestamp);
                self.dispatch(packet).await?;
            }
            UtpEvent::Tick => {
                println!("TICK RECEIVED {} {:?}", self.ntimeout, Instant::now());
                if Instant::now() > self.timeout {
                    if self.ntimeout >= 3 {
                        println!("RETURN ERRR", );
                        return Err(Error::new(ErrorKind::TimedOut, "utp connect timed out").into());
                    }
                    self.on_timeout().await;
                    // Resend packets
                } else {
                    // self.ntimeout = 0;
                }
            }
        }
        Ok(())
    }

    async fn on_timeout(&mut self) {
        let utp_state = self.state.utp_state();

        use UtpState::*;

        match utp_state {
            SynSent => {
                self.writer.send(WriterCommand::SendSyn).await;
            }
            Connected => {

            }
            _ => {}
        }

        // If packet inflight
        self.ntimeout += 1;

        self.timeout = Instant::now() + Duration::from_secs(1);
    }

    async fn dispatch(&mut self, packet: PacketRef<'_>) -> Result<()> {
        println!("DISPATCH HEADER: {:?}", packet.header());

        let delay = packet.received_at.delay(packet.get_timestamp());
        // self.delay = Delay::since(packet.get_timestamp());

        let utp_state = self.state.utp_state();

        self.ntimeout = 0;
        self.timeout = Instant::now() + Duration::from_secs(1);

        match (packet.get_type()?, utp_state) {
            (PacketType::Syn, UtpState::None) => {
                println!("RECEIVED SYN {:?}", self.addr);
                // Set self.remote
                let connection_id = packet.get_connection_id();

                self.state.set_utp_state(UtpState::Connected);
                self.state.set_recv_id(connection_id + 1);
                self.state.set_send_id(connection_id);
                self.state.set_seq_number(SequenceNumber::random());
                self.state.set_ack_number(packet.get_seq_number());
                self.state.set_remote_window(packet.get_window_size());

                self.writer.send(WriterCommand::SendPacket {
                    packet: Packet::new_type(PacketType::State)
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
            }
            (PacketType::State, UtpState::Connected) => {
                let remote_window = self.state.remote_window();

                if remote_window != packet.get_window_size() {
                    panic!("WINDOW SIZE CHANGED {:?}", packet.get_window_size());
                }

                //self.handle_state(packet).await?;
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

}

#[derive(Debug)]
struct UtpReader {
    rcv: Receiver<ReaderCommand>,
    send: Sender<ReaderResult>,
    /// Do not await while locking the state
    /// The await could block and lead to a deadlock state
    state: Arc<RwLock<State>>,
}

impl UtpReader {
    fn new(
        rcv: Receiver<ReaderCommand>,
        send: Sender<ReaderResult>,
        state: Arc<RwLock<State>>,
    ) -> UtpReader {
        UtpReader { rcv, send, state }
    }

    async fn start(self) {

    }
}

impl Drop for UtpReader {
    fn drop(&mut self) {
        println!("UtpReader Dropped", );
    }
}

#[derive(Debug)]
struct ReaderCommand {
    length: usize
}

#[derive(Debug)]
struct ReaderResult {
    data: Vec<u8>
}

#[derive(Debug)]
struct UtpWriter {
    socket: Arc<UdpSocket>,
    addr: SocketAddr,
    command: Receiver<WriterCommand>,
    /// Do not await while locking the state
    /// The await could block and lead to a deadlock state
    state: Arc<State>
    // result: Sender<WriterResult>,
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
        packet: Packet
    },
    /// Syn is a special case
    SendSyn,
    ResendPacket {

    },
    Close
}

// #[derive(Debug)]
// struct WriterResult {
//     data: Vec<u8>
// }

impl UtpWriter {
    pub fn new(
        socket: Arc<UdpSocket>,
        addr: SocketAddr,
        command: Receiver<WriterCommand>,
        state: Arc<State>,
    ) -> UtpWriter {
        UtpWriter { socket, addr, command, state }
    }

    pub async fn start(mut self) {
        use WriterCommand::*;

        while let Some(cmd) = self.command.recv().await {
            match cmd {
                WriteData { ref data } => {
                    self.send(data).await.unwrap()
                },
                SendPacket { packet } => {
                    self.send_packet(packet).await.unwrap();
                }
                ResendPacket { } => {}
                SendSyn => {
                    self.send_syn().await.unwrap();
                }
                Close => {
                    return;
                }
            }
        }
    }

    async fn send(&mut self, data: &[u8]) -> Result<()> {
        Ok(())
    }

    async fn send_packet(&mut self, mut packet: Packet) -> Result<()> {
        let ack_number = self.state.ack_number();
        let seq_number = self.state.increment_seq();
        let send_id = self.state.send_id();

        packet.set_ack_number(ack_number);
        packet.set_packet_seq_number(seq_number);
        packet.set_connection_id(send_id);
        packet.set_window_size(1_048_576);

        println!("SENDING NEW PACKET ! {:?}", packet);

        // println!("SENDING {:?}", &*packet);
        packet.update_timestamp();

        self.socket.send_to(packet.as_bytes(), self.addr).await?;

        {
            let mut inflight_packets = self.state.inflight_packets.write().await;
            inflight_packets.push_back(packet);
        }

        Ok(())
    }

    async fn send_syn(&mut self) -> Result<()> {
        let seq_number = self.state.increment_seq();
        let recv_id = self.state.recv_id();

        let mut packet = Packet::syn();
        packet.set_connection_id(recv_id);
        packet.set_seq_number(seq_number);
        packet.set_window_size(1_048_576);

        println!("SENDING {:#?}", packet);

        packet.update_timestamp();
        self.socket.send_to(packet.as_bytes(), self.addr).await?;

        self.state.set_utp_state(UtpState::SynSent);
        {
            let mut inflight_packets = self.state.inflight_packets.write().await;
            inflight_packets.push_back(packet);
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct UtpStream {
    // reader_command: Sender<ReaderCommand>,
    // reader_result: Receiver<ReaderResult>,
    writer_command: Sender<WriterCommand>,
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
        self.writer_command.send(WriterCommand::WriteData {
            data
        }).await;
    }
}
