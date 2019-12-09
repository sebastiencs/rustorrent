
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
use std::time::Duration;

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

#[derive(Debug)]
struct State {
    utp_state: UtpState,
    ack_number: SequenceNumber,
    seq_number: SequenceNumber,
    recv_id: ConnectionId,
    send_id: ConnectionId,
    /// advirtised window from the remote
    remote_window: u32,
    cwnd: u32,

    /// Packets sent but we didn't receive an ack for them
    inflight_packets: VecDeque<Packet>,

    /// Notify we're connected
    /// True if connected
    on_connected: Option<Sender<bool>>,
}

impl State {
    fn with_utp_state(utp_state: UtpState) -> State {
        State {
            utp_state,
            ..Default::default()
        }
    }
}

impl Default for State {
    fn default() -> State {
        let (recv_id, send_id) = ConnectionId::make_ids();

        State {
            recv_id,
            send_id,
            utp_state: UtpState::None,
            ack_number: SequenceNumber::zero(),
            seq_number: SequenceNumber::random(),
            // delay: Delay::default(),
            // current_delays: VecDeque::with_capacity(16),
            // last_rollover: Instant::now(),
            cwnd: INIT_CWND * MSS,
            // congestion_timeout: Duration::from_secs(1),
            // flight_size: 0,
            // srtt: 0,
            // rttvar: 0,
            inflight_packets: VecDeque::with_capacity(64),
            remote_window: INIT_CWND * MSS,
            // delay_history: DelayHistory::new(),
            // ack_duplicate: 0,
            // last_ack: SequenceNumber::zero(),
            // lost_packets: VecDeque::with_capacity(100),
            // nlost: 0,
            on_connected: None
        }
    }
}

const BUFFER_CAPACITY: usize = 1500;

pub struct UtpListener {
    v4: Arc<UdpSocket>,
    v6: Arc<UdpSocket>,
    /// The hashmap might be modified by different tasks so we wrap it in a RwLock
    streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>,
    // sender: Sender<UtpEvent>,
    // _recv: Receiver<UtpEvent>,
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

        let mut state = State::with_utp_state(UtpState::MustConnect);
        state.on_connected = Some(on_connected);
        let state = Arc::new(RwLock::new(state));

        let (sender, receiver) = channel(10);
        let manager = UtpManager::new_with_state(socket, sockaddr, receiver, state);

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

#[derive(Debug)]
struct UtpManager {
    socket: Arc<UdpSocket>,
    recv: Receiver<UtpEvent>,
    /// Do not await while locking the state
    /// The await could block and lead to a deadlock state
    state: Arc<RwLock<State>>,
    addr: SocketAddr,
    writer: Sender<WriterCommand>,
    reader: Sender<ReaderCommand>,
    _reader_rcv: Receiver<ReaderCommand>,
    reader_result: Sender<ReaderResult>,
    _reader_result_rcv: Receiver<ReaderResult>,
}

// pub struct IncomingBytes {
//     pub buffer: Vec<u8>,
//     pub timestamp: Timestamp
// }

pub enum UtpEvent {
    IncomingBytes {
        buffer: Vec<u8>,
        timestamp: Timestamp
    },
    Tick
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
        state: Arc<RwLock<State>>,
    ) -> UtpManager {
        let (writer, writer_rcv) = channel(10);

        let writer_actor = UtpWriter::new(socket.clone(), addr, writer_rcv, Arc::clone(&state));
        task::spawn(async move {
            writer_actor.start().await;
        });

        let (reader, _reader_rcv) = channel(10);
        let (reader_result, _reader_result_rcv) = channel(10);

        let reader_actor = UtpReader::new(_reader_rcv.clone(), reader_result.clone(), Arc::clone(&state));
        task::spawn(async move {
            reader_actor.start().await;
        });

        UtpManager {
            socket,
            addr,
            recv,
            writer,
            state,
            reader,
            _reader_rcv,
            reader_result,
            _reader_result_rcv
        }
    }

    pub fn get_stream(&self) -> UtpStream {
        UtpStream {
            reader_command: self.reader.clone(),
            reader_result: self._reader_result_rcv.clone(),
            writer_command: self.writer.clone()
        }
    }

    async fn start(self) {
        self.ensure_connected().await;
        while let Some(incoming) = self.recv.recv().await {
            self.process_incoming(incoming).await;
        }
    }

    async fn ensure_connected(&self) {
        let state = {
            let state = self.state.read().await;
            state.utp_state
        };

        if state != UtpState::MustConnect {
            return;
        }

        self.writer.send(WriterCommand::SendSyn).await;
    }

    async fn process_incoming(&self, event: UtpEvent) -> Result<()> {
        match event {
            UtpEvent::IncomingBytes { buffer, timestamp } => {
                let packet = PacketRef::ref_from_incoming(&buffer, timestamp);
                self.dispatch(packet).await?;
            }
            UtpEvent::Tick => {
                println!("TICK RECEIVED", );
            }
        }
        Ok(())
    }

    async fn dispatch(&self, packet: PacketRef<'_>) -> Result<()> {
        println!("DISPATCH HEADER: {:?}", packet.header());

        let delay = packet.received_at.delay(packet.get_timestamp());
        // self.delay = Delay::since(packet.get_timestamp());

        let (utp_state, remote_window) = {
            let state = self.state.read().await;
            (state.utp_state, state.remote_window)
        };

        match (packet.get_type()?, utp_state) {
            (PacketType::Syn, UtpState::None) => {
                println!("RECEIVED SYN {:?}", self.addr);
                // Set self.remote
                let connection_id = packet.get_connection_id();

                {
                    let mut state = self.state.write().await;
                    state.utp_state = UtpState::Connected;
                    state.recv_id = connection_id + 1;
                    state.send_id = connection_id;
                    state.seq_number = SequenceNumber::random();
                    state.ack_number = packet.get_seq_number();
                    state.remote_window = packet.get_window_size();
                }

                // self.state.recv_id = connection_id + 1;
                // self.state.send_id = connection_id;
                // self.state.seq_number = SequenceNumber::random();
                // self.state.ack_number = packet.get_seq_number();
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
                let notify = {
                    let mut state = self.state.write().await;
                    state.utp_state = UtpState::Connected;
                    state.ack_number = packet.get_seq_number() - 1;
                    state.remote_window = packet.get_window_size();
                    println!("CONNECTED !", );
                    state.on_connected.take()
                };
                if let Some(notify) = notify {
                    notify.send(true).await;
                };

            }
            (PacketType::State, UtpState::Connected) => {
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
    state: Arc<RwLock<State>>
    // result: Sender<WriterResult>,
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

    }
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
        state: Arc<RwLock<State>>,
    ) -> UtpWriter {
        UtpWriter { socket, addr, command, state }
    }

    pub async fn start(mut self) {
        while let Some(cmd) = self.command.recv().await {
            match cmd {
                WriterCommand::WriteData { ref data } => {
                    self.send(data).await.unwrap()
                },
                WriterCommand::SendPacket { packet } => {
                    self.send_packet(packet).await.unwrap();
                }
                WriterCommand::ResendPacket { } => {}
                WriterCommand::SendSyn => {
                    self.send_syn().await.unwrap();
                }
            }
        }
    }

    async fn send(&mut self, data: &[u8]) -> Result<()> {
        Ok(())
    }

    async fn send_packet(&mut self, mut packet: Packet) -> Result<()> {
        let (ack_number, seq_number, send_id) = {
            let state = self.state.read().await;
            (state.ack_number, state.seq_number, state.send_id)
        };

        packet.set_ack_number(ack_number);
        packet.set_packet_seq_number(seq_number);
        packet.set_connection_id(send_id);
        packet.set_window_size(1_048_576);

        println!("SENDING NEW PACKET ! {:?}", packet);

        // println!("SENDING {:?}", &*packet);
        packet.update_timestamp();

        self.socket.send_to(packet.as_bytes(), self.addr).await?;

        {
            let mut state = self.state.write().await;
            state.seq_number += 1;
            state.inflight_packets.push_back(packet);
        }

        Ok(())
    }

    async fn send_syn(&mut self) -> Result<()> {
        let (seq_number, recv_id) = {
            let state = self.state.read().await;
            (state.seq_number, state.recv_id)
        };

        let mut packet = Packet::syn();
        packet.set_connection_id(recv_id);
        packet.set_seq_number(seq_number);
        packet.set_window_size(1_048_576);

        println!("SENDING {:#?}", packet);

        packet.update_timestamp();
        self.socket.send_to(packet.as_bytes(), self.addr).await?;

        {
            let mut state = self.state.write().await;
            state.seq_number += 1;
            state.inflight_packets.push_back(packet);
            state.utp_state = UtpState::SynSent;
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct UtpStream {
    reader_command: Sender<ReaderCommand>,
    reader_result: Receiver<ReaderResult>,
    writer_command: Sender<WriterCommand>,
}

impl UtpStream {
    pub async fn read(&self, data: &mut [u8]) {
        self.reader_command.send(ReaderCommand {
            length: data.len()
        }).await;

        self.reader_result.recv().await;
    }

    pub async fn write(&self, data: &[u8]) {
        let data = Vec::from_slice(data);
        self.writer_command.send(WriterCommand::WriteData {
            data
        }).await;
    }
}
