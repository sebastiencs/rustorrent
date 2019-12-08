
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
        }
    }
}

const BUFFER_CAPACITY: usize = 1500;

pub struct UtpListener {
    v4: Arc<UdpSocket>,
    v6: Arc<UdpSocket>,
    /// The buffer is used only by one task and won't be borrowed more than once
    buffer_v4: RefCell<Vec<u8>>,
    /// The buffer is used only by one task and won't be borrowed more than once
    buffer_v6: RefCell<Vec<u8>>,
    /// The hashmap might be modified by different tasks so we wrap it in a RwLock
    streams: RwLock<HashMap<SocketAddr, Sender<IncomingBytes>>>,
    // sender: Sender<IncomingBytes>,
    // _recv: Receiver<IncomingBytes>,
}

enum IncomingEvent {
    V4((usize, SocketAddr)),
    V6((usize, SocketAddr)),
}

impl UtpListener {
    pub async fn new(port: u16) -> Result<UtpListener> {
        let v4 = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port)).await?;
        let v6 = UdpSocket::bind(SocketAddrV6::new(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1), port, 0, 0)).await?;

        let mut buffer_v4 = Vec::with_capacity(BUFFER_CAPACITY);
        let mut buffer_v6 = Vec::with_capacity(BUFFER_CAPACITY);

        unsafe {
            buffer_v4.set_len(BUFFER_CAPACITY);
            buffer_v6.set_len(BUFFER_CAPACITY);
        }

        Ok(UtpListener {
            v4: Arc::new(v4),
            v6: Arc::new(v6),
            buffer_v4: RefCell::new(buffer_v4),
            buffer_v6: RefCell::new(buffer_v6),
            streams: Default::default(),
        })
    }

    fn get_matching_socket(&self, sockaddr: &SocketAddr) -> Arc<UdpSocket> {
        if sockaddr.is_ipv4() {
            Arc::clone(&self.v4)
        } else {
            Arc::clone(&self.v6)
        }
    }

    async fn connect(&self, sockaddr: SocketAddr) -> Result<()> {

        let socket = self.get_matching_socket(&sockaddr);

        let mut buffer = [0; 1500];
        let mut len_addr = None;

        let mut state = State::default();

        let mut header = Packet::syn();
        header.set_connection_id(state.recv_id);
        header.set_seq_number(state.seq_number);
        header.set_window_size(1_048_576);
        state.seq_number += 1;

        for _ in 0..3 {
            header.update_timestamp();
            println!("SENDING {:#?}", header);

            socket.send(header.as_bytes()).await?;

            match socket.recv_from_timeout(&mut buffer, Duration::from_secs(1)).await {
                Ok((n, addr)) => {
                    len_addr = Some((n, addr));
                    // self.remote = Some(addr);
                    state.utp_state = UtpState::SynSent;
                    break;
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    continue;
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        };

        if let Some((len, addr)) = len_addr {
            println!("CONNECTED", );
            let packet = PacketRef::ref_from_buffer(&buffer[..len])?;
            // self.dispatch(packet).await?;

            // let manager = UtpManager::new(socket, sockaddr, receiver);

            let (sender, receiver) = channel(10);
            let manager = UtpManager::new(socket, sockaddr, receiver);

            {
                let mut streams = self.streams.write().await;
                streams.insert(sockaddr, sender.clone());
            }

            task::spawn(async move {
                manager.start().await
            });


            // let udp_clone = udp.try_clone()?;
            // let udp_clone2 = udp.try_clone()?;

            task::spawn(async move {

            });

            return Ok(());
        }

        Err(Error::new(ErrorKind::TimedOut, "connect timed out").into())

    }

    async fn new_connection(&self, sockaddr: SocketAddr) -> Sender<IncomingBytes> {
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

    pub async fn start(&self) -> Result<()> {
        self.process_incoming().await
    }

    async fn poll(&self) -> Result<IncomingEvent> {
        let mut buffer_v4 = self.buffer_v4.borrow_mut();
        let mut buffer_v6 = self.buffer_v6.borrow_mut();
        let v4 = self.v4.recv_from(buffer_v4.as_mut_slice());
        let v6 = self.v6.recv_from(buffer_v6.as_mut_slice());
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

    async fn poll_event(&self) -> Result<IncomingEvent> {
        loop {
            match self.poll().await {
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

        loop {
            let (buffer, addr) = match self.poll_event().await? {
                V4((size, addr)) => {
                    (Vec::from_slice(&self.buffer_v4.borrow()[..size]), addr)
                },
                V6((size, addr)) => {
                    (Vec::from_slice(&self.buffer_v4.borrow()[..size]), addr)
                },
            };

            let timestamp = Timestamp::now();

            println!("INCOMING {:?} {:?}", buffer.len(), addr);

            if buffer.len() < HEADER_SIZE {
                continue;
            }

            let incoming = IncomingBytes { buffer, timestamp };

            {
                if let Some(addr) = self.streams.read().await.get(&addr) {
                    addr.send(incoming).await;
                    continue;
                }
            }

            let packet = PacketRef::ref_from_incoming(&incoming);

            if let Ok(PacketType::Syn) = packet.get_type() {
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
    recv: Receiver<IncomingBytes>,
    state: Arc<RwLock<State>>,
    addr: SocketAddr,
    writer: Sender<WriterCommand>,
}

pub struct IncomingBytes {
    pub buffer: Vec<u8>,
    pub timestamp: Timestamp
}

impl UtpManager {
    fn new(
        socket: Arc<UdpSocket>,
        addr: SocketAddr,
        recv: Receiver<IncomingBytes>
    ) -> UtpManager {
        let (writer, writer_rcv) = channel(10);

        let state = Default::default();

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
        }
    }

    pub fn get_stream(&self) -> UtpStream {

        // #[derive(Debug)]
        // struct UtpStream {
        //     reader_command: Sender<ReaderCommand>,
        //     reader_result: Receiver<ReaderResult>,
        //     writer_command: Sender<WriterCommand>,
        //     // writer_result: Receiver<WriterResult>,
        // }

        // UtpStream {

        // }

    }

    async fn start(self) {
        while let Some(incoming) = self.recv.recv().await {
            self.process_incoming(incoming).await;
        }
    }

    async fn process_incoming(&self, incoming: IncomingBytes) -> Result<()> {
        let packet = PacketRef::ref_from_incoming(&incoming);
        self.dispatch(packet).await?;
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
                let mut state = self.state.write().await;
                state.utp_state = UtpState::Connected;
                state.ack_number = packet.get_seq_number() - 1;
                state.remote_window = packet.get_window_size();
                println!("CONNECTED !", );
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
    socket: UdpSocket
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
        // result: Sender<WriterResult>,
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
        packet.update_timestamp();

        println!("SENDING NEW PACKET ! {:?}", packet);

        // println!("SENDING {:?}", &*packet);

        self.socket.send_to(packet.as_bytes(), self.addr).await?;

        {
            let mut state = self.state.write().await;
            state.seq_number += 1;
            state.inflight_packets.push_back(packet);
        }

        Ok(())
    }

    // async fn send_result(&mut self) {

    // }
}

#[derive(Debug)]
struct UtpStream {
    reader_command: Sender<ReaderCommand>,
    reader_result: Receiver<ReaderResult>,
    writer_command: Sender<WriterCommand>,
    // writer_result: Receiver<WriterResult>,
}

impl UtpStream {
    pub fn read(&self) {

    }

    pub fn write(&self) {

    }
}
