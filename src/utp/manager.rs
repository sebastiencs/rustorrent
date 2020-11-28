use async_channel::{unbounded, Receiver, Sender};
use futures::StreamExt;
use socket2::Socket;
use tokio::{
    io::{Error, ErrorKind},
    sync::oneshot,
    task,
};
// use tokio::net::UdpSocket;
use fixed::types::I48F16;
use std::{collections::VecDeque, net::SocketAddr, pin::Pin, sync::Arc};

// use tokio::sync::mpsc::{Receiver as TokioReceiver, channel, error::TrySendError as TokioTrySendError};

// use crate::utils::Map;

use super::{
    stream::ReceivedData,
    udp_socket::{FdGuard, MyUdpSocket as UdpSocket},
};
use shared_arena::{ArenaBox, SharedArena};
//use super::writer::{UtpWriter, WriterCommand, WriterUserCommand};
use super::{
    ack_bitfield::AckedBitfield,
    stream::{State, UtpStream},
};

use super::{
    DelayHistory, Packet, PacketType, Result, SelectiveAckBit, SequenceNumber, Timestamp, UtpError,
    UtpState, HEADER_SIZE, MIN_CWND, MSS, TARGET, UDP_IPV4_MTU, UDP_IPV6_MTU,
};

use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const RESEND: u8 = 1 << 0;
const RESEND_ONLY_LOST: u8 = 1 << 1;
const SEND_STATE: u8 = 1 << 2;

struct NeedToSend {
    detail: u8,
}

impl NeedToSend {
    fn new() -> NeedToSend {
        NeedToSend { detail: 0 }
    }

    fn has(&self, data: u8) -> bool {
        self.detail & data != 0
    }

    fn set(&mut self, set: u8) {
        self.detail |= set;
    }

    fn unset(&mut self, unset: u8) {
        self.detail &= !unset;
    }

    fn is_empty(&self) -> bool {
        self.detail == 0
    }
}

pub(super) struct UtpManager {
    socket: Arc<UdpSocket>,
    recv: Pin<Box<Receiver<UtpEvent>>>,
    // recv: Receiver<UtpEvent>,
    sender: Sender<UtpEvent>,
    /// Do not await while locking the state
    /// The await could block and lead to a deadlock state
    state: State,
    addr: SocketAddr,
    // writer_user: Option<Sender<WriterUserCommand>>,
    // user_cmd: Receiver<WriterUserCommand>,
    //user_cmd_sender: Option<Sender<WriterUserCommand>>,
    // writer: Sender<WriterCommand>,
    on_receive: Sender<ReceivedData>,
    on_receive_recv: Option<Receiver<ReceivedData>>,

    ack_bitfield: AckedBitfield,

    packet_arena: Arc<SharedArena<Packet>>,

    to_send: VecDeque<ArenaBox<Packet>>,
    need_send: NeedToSend,

    timeout: Instant,
    /// Number of consecutive times we've been timeout
    /// After 3 times we close the socket
    ntimeout: usize,

    on_connected: Option<oneshot::Sender<(UtpStream, SocketAddr)>>,

    ack_duplicate: u8,
    last_ack: SequenceNumber,

    delay_history: DelayHistory,

    tmp_packet_losts: Vec<SequenceNumber>,

    nlost: usize,

    slow_start: bool,

    next_cwnd_decrease: Instant,

    rtt: usize,
    rtt_var: usize,
    rto: usize,

    received: usize,
    // received_data: Map<SequenceNumber, Box<[u8]>>
}

// #[derive(Debug)]
pub enum UtpEvent {
    IncomingPacket { packet: ArenaBox<Packet> },
    Timeout,
    Writable,
    UserWrite { data: Box<[u8]> },
    // IncomingBytes {
    //     buffer: Vec<u8>,
    //     timestamp: Timestamp
    // },
    // Tick
}

impl std::fmt::Debug for UtpEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            UtpEvent::UserWrite { data } => write!(f, "UserWrite {:?}", data.len()),
            UtpEvent::Writable => write!(f, "Writable"),
            UtpEvent::Timeout => write!(f, "Timeout"),
            UtpEvent::IncomingPacket { .. } => write!(f, "IncomingPacket"),
        }
    }
}

#[derive(Debug)]
enum SendPush {
    Front,
    Back,
}

impl Drop for UtpManager {
    fn drop(&mut self) {
        println!("DROP UTP MANAGER",);
        // if let Some(on_connected) = self.on_connected.take() {
        //     task::spawn(async move {
        //         on_connected.send(None).await
        //     });
        // };
    }
}

impl UtpManager {
    pub(super) fn new_with_state(
        socket: Arc<UdpSocket>,
        addr: SocketAddr,
        recv: Receiver<UtpEvent>,
        sender: Sender<UtpEvent>,
        state: State,
        packet_arena: Arc<SharedArena<Packet>>,
        on_connected: Option<oneshot::Sender<(UtpStream, SocketAddr)>>,
    ) -> UtpManager {
        let (on_receive_sender, on_receive) = unbounded();

        UtpManager {
            socket,
            addr,
            recv: Box::pin(recv),
            sender,
            state,
            on_receive: on_receive_sender,
            on_receive_recv: Some(on_receive),
            packet_arena,
            to_send: VecDeque::new(),
            need_send: NeedToSend::new(),
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
            rto: 0,
            ack_bitfield: AckedBitfield::default(),
            received: 0,
        }
    }

    pub fn get_stream(&mut self) -> Option<UtpStream> {
        Some(UtpStream::new(
            self.on_receive_recv.take()?,
            self.sender.clone(),
        ))
    }

    #[allow(clippy::needless_lifetimes)]
    async fn receive_timeout<'a>(
        &mut self,
        socket: &'a UdpSocket,
    ) -> Result<(UtpEvent, Option<FdGuard<'a, Socket>>)> {
        // let need_to_send = self.to_send_detail != 0 || !self.to_send.is_empty();
        let need_to_send = !self.need_send.is_empty() || !self.to_send.is_empty();

        return tokio::select! {
            recv = self.recv.next() => {
                recv.map(|v| (v, None)).ok_or(UtpError::RecvClosed)
            }
            guard = socket.writable(), if need_to_send => {
                guard.map(|g| (UtpEvent::Writable, Some(g)))
                     .map_err(UtpError::IO)
            }
        };

        // let fun = |cx: &mut Context<'_>| {
        //     if let Poll::Ready(v) = self.recv.as_mut().poll_next(cx) {
        //         return Poll::Ready(v.map(|v| (v, None)).ok_or_else(|| UtpError::RecvClosed))
        //     }

        //     if self.to_send_detail != 0 || !self.to_send.is_empty() {
        //         let writer = socket.writable();
        //         pin_mut!(writer);

        //         if let Poll::Ready(Ok(guard)) = writer.as_mut().poll(cx) {
        //             return Poll::Ready(Ok((UtpEvent::Writable, Some(guard))))
        //         }
        //     }

        //     Poll::Pending
        // };

        // return future::poll_fn(fun).await;
    }

    pub(super) async fn start(mut self) -> Result<()> {
        use UtpError::SendWouldBlock;

        self.ensure_connected();
        let socket = self.socket.clone();

        while let Ok((incoming, mut guard)) = { self.receive_timeout(&socket).await } {
            let result = self.process_incoming(incoming);

            match (result, &mut guard) {
                (Err(SendWouldBlock), Some(guard)) => {
                    guard.clear_ready();
                }
                (Err(SendWouldBlock), None) => {}
                (Err(e), _) => return Err(e),
                (Ok(_), _) => {}
            }
        }

        println!("manager finished");
        Ok(())
    }

    fn ensure_connected(&mut self) {
        let state = self.state.utp_state();

        if state != UtpState::MustConnect {
            return;
        }

        self.send_packet(PacketType::Syn);
    }

    fn process_incoming(&mut self, event: UtpEvent) -> Result<()> {
        match event {
            UtpEvent::IncomingPacket { packet } => {
                // println!("== Incoming received {:?}", packet);
                let res = self.dispatch(packet);
                // println!("result: {:?}", res);
                match res {
                    Err(UtpError::PacketLost) => {
                        self.resend_packets(true)?;
                    }
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
            }
            UtpEvent::Timeout => {
                // println!("== timeout {:?}", self.ntimeout);
                if Instant::now() > self.timeout {
                    self.on_timeout()?;
                }
            }
            UtpEvent::Writable => {
                if self.need_send.has(SEND_STATE) {
                    self.send_state();
                }
                if self.need_send.has(RESEND | RESEND_ONLY_LOST) {
                    let only_lost = self.need_send.has(RESEND_ONLY_LOST);
                    self.resend_packets(only_lost)?;
                }
                // if self.to_send_detail != 0 {
                //     self.resend_packets(
                //         self.to_send_detail | TO_SEND_PACKETS_RESEND_ONLY_LOST
                //             == TO_SEND_PACKETS_RESEND_ONLY_LOST,
                //     )?;
                // }
                // TODO: Don't push front here
                while let Some(packet) = self.to_send.pop_front() {
                    if !self.is_window_large_enough(packet.size()) {
                        self.to_send.push_front(packet);
                        break;
                    }
                    // println!("write {:?}", packet.get_seq_number());
                    self.send_packet_inner(packet, SendPush::Front)?;
                }
            }
            UtpEvent::UserWrite { data } => {
                self.send(&data)?;
            }
        }
        Ok(())
    }

    fn on_timeout(&mut self) -> Result<()> {
        use UtpState::*;

        let utp_state = self.state.utp_state();

        if (utp_state == SynSent && self.ntimeout >= 3) || self.ntimeout >= 7 {
            return Err(Error::new(ErrorKind::TimedOut, "utp timed out").into());
        }

        match utp_state {
            SynSent | Connected => {
                if self.state.inflight_size() > 0 {
                    // println!("resend here");

                    // let in_flight: Vec<_> = self.state.inflight_packets.keys().collect();
                    // println!("in_flight={:?}", in_flight);

                    self.resend_packets(false)?;
                }
            }
            _ => {}
        }

        if self.state.inflight_size() > 0 {
            self.ntimeout += 1;
        } else if self.to_send.is_empty() && self.state.inflight_size() == 0 {
            println!(
                "NLOST {:?} in_flight={} to_send={}",
                self.nlost,
                self.state.inflight_size(),
                self.to_send.len()
            );
            // self.notify_finish();
            // println!("RTO {:?} RTT {:?} RTT_VAR {:?}", self.rto, self.rtt, self.rtt_var);
        }

        self.timeout = Instant::now() + Duration::from_millis(self.rto as u64);

        Ok(())
    }

    fn notify_finish(&self) {
        println!(
            "FINISH flight={} to_send={}",
            self.state.inflight_size(),
            self.to_send.len()
        );
        let on_receive = self.on_receive.clone();
        task::spawn(async move {
            on_receive.send(ReceivedData::Done).await.unwrap();
        });
    }

    fn dispatch(&mut self, packet: ArenaBox<Packet>) -> Result<()> {
        // println!("DISPATCH HEADER: {:?}", (*packet));

        let packet_type = packet.get_type()?;

        let _delay = packet.received_at().delay(packet.get_timestamp());
        // self.delay = Delay::since(packet.get_timestamp());

        let utp_state = self.state.utp_state();

        // println!("ntimeout reset {}", self.ntimeout);
        self.ntimeout = 0;
        self.timeout = Instant::now() + Duration::from_millis(self.rto as u64);

        // println!("STATE {:?}", (&packet_type, utp_state));

        match (packet_type, utp_state) {
            (PacketType::Syn, UtpState::None) => {
                //println!("RECEIVED SYN {:?}", self.addr);
                let connection_id = packet.get_connection_id();

                self.state.set_utp_state(UtpState::SynReceived);
                self.state.set_recv_id(connection_id + 1);
                self.state.set_send_id(connection_id);
                self.state.set_seq_number(SequenceNumber::random());
                // self.state.set_ack_number(packet.get_seq_number());
                self.ack_bitfield.init(packet.get_seq_number());
                self.state.set_remote_window(packet.get_window_size());

                self.send_state();
                //self.send_packet(PacketType::State);

                // self.writer.send(WriterCommand::SendPacket {
                //     packet_type: PacketType::State
                // }).await.unwrap();
            }
            (PacketType::Syn, _) => {
                // Ignore syn packets when we are already connected
            }
            (PacketType::State, UtpState::SynSent) => {
                // Related:
                // https://engineering.bittorrent.com/2015/08/27/drdos-udp-based-protocols-and-bittorrent/
                // https://www.usenix.org/system/files/conference/woot15/woot15-paper-adamsky.pdf
                // https://github.com/bittorrent/libutp/commit/13d33254262d46b638d35c4bc1a2f76cea885760

                self.state.set_utp_state(UtpState::Connected);
                // self.state.set_ack_number(packet.get_seq_number() - 1);
                self.ack_bitfield.ack(packet.get_seq_number() - 1);
                self.state.set_remote_window(packet.get_window_size());

                if let Some(notify) = self.on_connected.take() {
                    let stream = self.get_stream().unwrap();
                    notify.send((stream, self.addr)).unwrap();
                };

                self.state.remove_packets(packet.get_ack_number());
            }
            (PacketType::State, UtpState::Connected) => {
                let remote_window = self.state.remote_window();

                if remote_window != packet.get_window_size() {
                    panic!("WINDOW SIZE CHANGED {:?}", packet.get_window_size());
                }

                self.handle_state(packet)?;
            }
            (PacketType::State, _) => {
                // Wrong Packet
            }
            // TODO: Match only Connected & SynReceived
            (PacketType::Data, _) => {
                if let Some(notify) = self.on_connected.take() {
                    self.state.set_utp_state(UtpState::Connected);
                    let stream = self.get_stream().unwrap();
                    notify.send((stream, self.addr)).unwrap();

                    let seq = packet.get_seq_number();
                    self.on_receive
                        .try_send(ReceivedData::FirstSequence { seq })
                        .unwrap();
                }

                let seq_num = packet.get_seq_number();
                self.ack_bitfield.ack(seq_num);

                self.received += packet.get_data().len();

                self.on_receive
                    .try_send(ReceivedData::Data { packet })
                    .unwrap();

                // println!("received={}", self.received);

                self.send_state();
            }
            (PacketType::Fin, _) => {
                return Err(UtpError::MustClose);
            }
            (PacketType::Reset, _) => {
                return Err(UtpError::MustClose);
            }
        }

        Ok(())
    }

    fn send_state(&mut self) {
        self.need_send.set(SEND_STATE);

        // TODO: Defer ack if recv is not empty

        let mut packet = Packet::new_type(PacketType::State);

        let ack_number = self.ack_bitfield.current();
        let seq_number = self.state.seq_number();
        let send_id = self.state.send_id();
        packet.set_ack_number(ack_number);
        packet.set_packet_seq_number(seq_number);
        packet.set_connection_id(send_id);
        packet.set_window_size(1_048_576);
        packet.update_timestamp();

        if let Some(bytes) = self.ack_bitfield.bytes_for_packet() {
            // println!("send with extensions {:?}", ack_number);
            packet.add_selective_acks(bytes);
        }

        // let mut ok = false;

        if self.try_send_to(&packet).is_ok() {
            self.need_send.unset(SEND_STATE);
            // ok = true;
            self.timeout = Instant::now() + Duration::from_millis(500);
        }

        // println!(
        //     "Send state success={} ext={} ack={:?} seq={:?}",
        //     ok,
        //     packet.has_extension(),
        //     ack_number,
        //     seq_number,
        // );
    }

    fn handle_state(&mut self, packet: ArenaBox<Packet>) -> Result<()> {
        let ack_number = packet.get_ack_number();
        let received_at = packet.received_at();

        // println!("ACK RECEIVED {:?} LAST_ACK {:?} DUP {:?} INFLIGHT {:?}",
        //          ack_number, self.last_ack, self.ack_duplicate, self.state.inflight_size());

        let in_flight = self.state.inflight_size();
        let mut bytes_newly_acked = 0;

        // let before = self.state.inflight_size();
        bytes_newly_acked += self.ack_packets(ack_number, received_at);
        // bytes_newly_acked += self.state.remove_packets(ack_number).await;
        //println!("PACKETS IN FLIGHT {:?}", self.inflight_packets.len());
        //        println!("PACKETS IN FLIGHT {:?}", self.inflight_packets.as_slices());

        // println!("IN_FLIGHT BEFORE {:?} AFTER {:?}", before, self.state.inflight_size());

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
                            bytes_newly_acked += self.ack_one_packet(seq_num, received_at);
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

            self.mark_packets_as_lost();

            // println!("MISSING FROM SACK {:?}", self.tmp_packet_losts);
            return Err(UtpError::PacketLost);
        }

        self.rto = 500.max(self.rtt + self.rtt_var * 4);

        // self.writer.send(WriterCommand::Acks).await.unwrap();

        Ok(())
    }

    fn update_rtt(
        packet: &Packet,
        ack_received_at: Timestamp,
        mut rtt: usize,
        mut rtt_var: usize,
    ) -> (usize, usize) {
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

    fn ack_packets(&mut self, ack_number: SequenceNumber, ack_received_at: Timestamp) -> usize {
        // Make temporary vars to make the borrow checker happy
        let (mut rtt, mut rtt_var) = (self.rtt, self.rtt_var);

        let mut size = 0;

        self.state.inflight_packets.retain(|_, p| {
            !p.is_seq_less_equal(ack_number) || {
                let (r, rv) = Self::update_rtt(p, ack_received_at, rtt, rtt_var);
                rtt = r;
                rtt_var = rv;
                size += p.size();

                // println!("remove from in_flight = {:?} with ack={:?}", p.get_seq_number(), ack_number);

                false
            }
        });

        self.rtt = rtt;
        self.rtt_var = rtt_var;

        self.state
            .in_flight
            .fetch_sub(size as u32, Ordering::AcqRel);
        size
    }

    fn ack_one_packet(&mut self, ack_number: SequenceNumber, ack_received_at: Timestamp) -> usize {
        // let mut inflight_packets = self.state.inflight_packets.write().await;
        let mut size = 0;

        if let Some(ref packet) = self.state.inflight_packets.remove(&ack_number) {
            let (r, rv) = Self::update_rtt(packet, ack_received_at, self.rtt, self.rtt_var);
            self.rtt = r;
            self.rtt_var = rv;
            size = packet.size();
            self.state
                .in_flight
                .fetch_sub(size as u32, Ordering::AcqRel);
        }

        size
    }

    fn mark_packets_as_lost(&mut self) {
        // Here
        // let mut inflight_packets = self.state.inflight_packets.write().await;
        for seq in &self.tmp_packet_losts {
            if let Some(packet) = self.state.inflight_packets.get_mut(seq) {
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

    fn send_packet(&mut self, packet_type: PacketType) {
        if packet_type == PacketType::Syn {
            self.send_syn().unwrap();
        } else {
            let packet = self.packet_arena.alloc(Packet::new_type(packet_type));
            self.send_packet_inner(packet, SendPush::Back).unwrap();
        }
    }

    fn send_packet_inner(&mut self, mut packet: ArenaBox<Packet>, place: SendPush) -> Result<()> {
        //let ack_number = self.state.ack_number();
        let ack_number = self.ack_bitfield.current();
        let seq_number = self.state.seq_number();
        //let seq_number = self.state.increment_seq();
        let send_id = self.state.send_id();

        packet.set_ack_number(ack_number);
        packet.set_packet_seq_number(seq_number);
        packet.set_connection_id(send_id);
        packet.set_window_size(1_048_576);

        // println!("SENDING NEW PACKET ! {:?} {:?}", seq_number, place);

        packet.update_timestamp();

        if let Err(e) = self.try_send_to(&packet) {
            match place {
                SendPush::Back => self.to_send.push_back(packet),
                SendPush::Front => self.to_send.push_front(packet),
            }
            // TODO: decrement seq here
            return Err(e);
        } else if packet.get_type()? != PacketType::State {
            // println!("add in flight {:?}", seq_number);
            self.state.add_packet_inflight(seq_number, packet);
        }

        self.state.increment_seq();

        Ok(())
    }

    fn try_send_to(&self, packet: &Packet) -> Result<()> {
        self.socket.try_send_to(packet.as_bytes(), self.addr)
    }

    fn send_syn(&mut self) -> Result<()> {
        let seq_number = self.state.increment_seq();
        let recv_id = self.state.recv_id();

        let mut packet = Packet::syn();
        packet.set_connection_id(recv_id);
        packet.set_packet_seq_number(seq_number);
        packet.set_window_size(1_048_576);

        //println!("SENDING {:#?}", packet);

        packet.update_timestamp();

        let packet = self.packet_arena.alloc(packet);

        if let Err(e) = self.try_send_to(&packet) {
            self.to_send.push_back(packet);
            return Err(e);
        }

        self.state.set_utp_state(super::UtpState::SynSent);

        self.state.add_packet_inflight(seq_number, packet);

        Ok(())
    }

    fn resend_packets(&mut self, only_lost: bool) -> Result<()> {
        // let mut inflight_packets = self.state.inflight_packets.write().await;

        // println!("RESENDING ONLY_LOST={:?} INFLIGHT_LEN={:?} CWND={:?}", only_lost, self.state.inflight_packets.len(), self.state.cwnd());

        // let mut resent = 0;

        // We copy the socket to satisfy the borrow checker
        let socket = self.socket.clone();
        //let ack_number = self.state.ack_number();
        let ack_number = self.ack_bitfield.current();

        self.need_send
            .set(if only_lost { RESEND_ONLY_LOST } else { RESEND });

        // packet_size + self.state.inflight_size() <= self.state.cwnd() as usize

        // println!("only_lost={} in flight size {}", only_lost, self.state.inflight_size());

        for packet in self.state.inflight_packets.values_mut() {
            if !only_lost || packet.lost {
                packet.set_ack_number(ack_number);
                packet.update_timestamp();

                // println!("== RESENDING {:?}", packet);
                // println!("== RESENDING {:?} CWND={:?} POS {:?} {:?}", start, self.cwnd, start, &**packet);

                // println!("resent {:?}", packet.get_seq_number());

                // resent += 1;
                socket.try_send_to(packet.as_bytes(), self.addr)?;

                packet.resent = true;
                packet.lost = false;
            }
        }

        self.need_send.unset(RESEND | RESEND_ONLY_LOST);
        // self.to_send_detail &= !(TO_SEND_PACKETS_RESEND | TO_SEND_PACKETS_RESEND_ONLY_LOST);
        // println!("resent n={} TO_SEND_DETAIL {}", resent, self.to_send_detail);

        // println!("RESENT: {:?}", resent);
        Ok(())
    }

    fn send(&mut self, data: &[u8]) -> Result<()> {
        let packet_size = self.packet_size();

        // We clone the arena to satisfy the borrow checker with self
        let arena = self.packet_arena.clone();

        let mut packets = data
            .chunks(packet_size)
            .map(|p| arena.alloc_with(|packet_uninit| Packet::new_in_place(packet_uninit, p)));

        // let mut sent = 0;

        while let Some(packet) = packets.next() {
            if !self.is_window_large_enough(packet.size()) {
                println!(
                    "window not large enough inflight={} cwnd={}",
                    self.state.inflight_size(),
                    self.state.cwnd()
                );
                self.to_send.push_back(packet);
                break;
            }
            if self.send_packet_inner(packet, SendPush::Back).is_err() {
                break;
            }
            // sent += 1;
        }

        self.to_send.extend(packets);

        // eprintln!("sent={}, to_send length={}", sent, self.to_send.len());

        Ok(())
    }

    fn is_window_large_enough(&self, packet_size: usize) -> bool {
        packet_size + self.state.inflight_size() <= self.state.cwnd() as usize
    }
}
