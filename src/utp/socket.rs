
use tokio::io::{ErrorKind, Error};
use rand::Rng;
use fixed::types::I48F16;

use std::net::SocketAddr;
use std::time::{Duration, Instant};
use std::{iter::Iterator, collections::VecDeque};
use std::iter;

use super::{
    ConnectionId, Result, UtpError, Packet, PacketType,
    Header, Delay, Timestamp, SequenceNumber, HEADER_SIZE,
    UDP_IPV4_MTU, UDP_IPV6_MTU, DelayHistory, RelativeDelay,
    SelectiveAckBit
};
use crate::udp_ext::WithTimeout;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum State {
    /// not yet connected
	None,
	/// sent a syn packet, not received any acks
	SynSent,
	/// syn-ack received and in normal operation
	/// of sending and receiving data
	Connected,
	/// fin sent, but all packets up to the fin packet
	/// have not yet been acked. We might still be waiting
	/// for a FIN from the other end
	FinSent,
    ///
    MustConnect
	// /// ====== states beyond this point =====
	// /// === are considered closing states ===
	// /// === and will cause the socket to ====
	// /// ============ be deleted =============
	// /// the socket has been gracefully disconnected
	// /// and is waiting for the client to make a
	// /// socket call so that we can communicate this
	// /// fact and actually delete all the state, or
	// /// there is an error on this socket and we're
	// /// waiting to communicate this to the client in
	// /// a callback. The error in either case is stored
	// /// in m_error. If the socket has gracefully shut
	// /// down, the error is error::eof.
	// ErrorWait,
	// /// there are no more references to this socket
	// /// and we can delete it
	// Delete
}

impl From<u8> for State {
    fn from(n: u8) -> State {
        match n {
            0 => State::None,
            1 => State::SynSent,
            2 => State::Connected,
            3 => State::FinSent,
            4 => State::MustConnect,
            _ => unreachable!()
        }
    }
}

impl From<State> for u8 {
    fn from(s: State) -> u8 {
        match s {
            State::None => 0,
            State::SynSent => 1,
            State::Connected => 2,
            State::FinSent => 3,
            State::MustConnect => 4,
        }
    }
}

pub const BASE_HISTORY: usize = 10;
pub const INIT_CWND: u32 = 2;
pub const MIN_CWND: u32 = 2;
/// Sender's Maximum Segment Size
/// Set to Ethernet MTU
pub const MSS: u32 = 1400;
pub const TARGET: i64 = 100_000; //100;
pub const GAIN: u32 = 1;
pub const ALLOWED_INCREASE: u32 = 1;

pub struct UtpSocket {
    local: SocketAddr,
    remote: Option<SocketAddr>,
    udp: UdpSocket,
    recv_id: ConnectionId,
    send_id: ConnectionId,
    state: State,
    ack_number: SequenceNumber,
    seq_number: SequenceNumber,
    delay: Delay,

    /// advirtised window from the remote
    remote_window: u32,

    /// Packets sent but we didn't receive an ack for them
    inflight_packets: VecDeque<Packet>,

    delay_history: DelayHistory,

    cwnd: u32,
    congestion_timeout: Duration,

    ack_duplicate: u8,
    last_ack: SequenceNumber,

    lost_packets: VecDeque<SequenceNumber>,

    nlost: usize,

    // /// SRTT (smoothed round-trip time)
    // srtt: u32,
    // /// RTTVAR (round-trip time variation)
    // rttvar: u32,
}

impl UtpSocket {
    fn new(local: SocketAddr, udp: UdpSocket) -> UtpSocket {
        let (recv_id, send_id) = ConnectionId::make_ids();

        // let mut base_delays = VecDeque::with_capacity(BASE_HISTORY);
        // base_delays.extend(iter::repeat(Delay::infinity()).take(BASE_HISTORY));

        UtpSocket {
            local,
            udp,
            recv_id,
            send_id,
            // base_delays,
            remote: None,
            state: State::None,
            ack_number: SequenceNumber::zero(),
            seq_number: SequenceNumber::random(),
            delay: Delay::default(),
            // current_delays: VecDeque::with_capacity(16),
            // last_rollover: Instant::now(),
            cwnd: INIT_CWND * MSS,
            congestion_timeout: Duration::from_secs(1),
            // flight_size: 0,
            // srtt: 0,
            // rttvar: 0,
            inflight_packets: VecDeque::with_capacity(64),
            remote_window: INIT_CWND * MSS,
            delay_history: DelayHistory::new(),
            ack_duplicate: 0,
            last_ack: SequenceNumber::zero(),
            lost_packets: VecDeque::with_capacity(100),
            nlost: 0,
        }
    }

    pub async fn bind(addr: SocketAddr) -> Result<UtpSocket> {
        let udp = UdpSocket::bind(addr).await?;

        Ok(Self::new(addr, udp))
    }

    /// Addr must match the ip familly of the bind address (ipv4 / ipv6)
    pub async fn connect(&mut self, addr: SocketAddr) -> Result<()> {
        if addr.is_ipv4() != self.local.is_ipv4() {
            return Err(UtpError::FamillyMismatch);
        }

        self.udp.connect(addr).await?;

        let mut buffer = [0; 1500];
        let mut len = None;

        let mut header = Packet::syn();
        header.set_connection_id(self.recv_id);
        header.set_seq_number(self.seq_number);
        header.set_window_size(1_048_576);
        self.seq_number += 1;

        for _ in 0..3 {
            header.update_timestamp();
            println!("SENDING {:#?}", header);

            self.udp.send(header.as_bytes()).await?;

            match self.udp.recv_from_timeout(&mut buffer, Duration::from_secs(1)).await {
                Ok((n, addr)) => {
                    len = Some(n);
                    self.remote = Some(addr);
                    self.state = State::SynSent;
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

        if let Some(len) = len {
            println!("CONNECTED", );
            let packet = PacketRef::ref_from_buffer(&buffer[..len])?;
            self.dispatch(packet).await?;
            return Ok(());
        }

        Err(Error::new(ErrorKind::TimedOut, "connect timed out").into())
    }

    pub async fn recv(&mut self, buffer: &mut [u8]) -> Result<()> {
        Ok(())
    }

    pub async fn send(&mut self, data: &[u8]) -> Result<()> {
        let packet_size = self.packet_size();
        let packets = data.chunks(packet_size).map(Packet::new);

        let mut size = 0;

        let mut packet_total = 0;

        for packet in packets {
            packet_total += 1;
            size += packet.payload_len();

            self.ensure_window_is_large_enough(packet.size()).await?;

            self.send_packet(packet).await?;
        }

        println!("\nDONE, WAITING NOW", );
        println!("TOTAL SENT {:?}\n", size);

        let res = self.wait_for_reception().await;

        println!("TOTAL={:?}", packet_total);
        println!("LOST={:?}", self.nlost);

        res
    }

    async fn ensure_window_is_large_enough(&mut self, packet_size: usize) -> Result<()> {
        while packet_size + self.inflight_size() > self.cwnd as usize {
            println!("BLOCKED BY CWND {:?} {:?} {:?}", packet_size, self.inflight_size(), self.cwnd);
            self.receive_and_handle_lost_packets().await?;
        }
        Ok(())
    }

    async fn receive_and_handle_lost_packets(&mut self) -> Result<()> {
        match self.receive_packet().await {
            Err(UtpError::PacketLost) => {
                // println!("PACKET LOST HEEER {:?}", self.last_ack);
                //self.resend_packet(self.last_ack + 1).await?;
                self.nlost += self.lost_packets.len();
                self.resend_lost_packets().await
            },
            x => x
        }
    }

    async fn resend_lost_packets(&mut self) -> Result<()> {
        println!("PROCEED TO RESEND {:?} PACKETS", self.lost_packets.len());
        while let Some(ack_num) = self.lost_packets.pop_front() {
            self.resend_packet(ack_num).await?;
        }

        let now = Timestamp::now();

        let expired_packets = self.inflight_packets
                                  .iter()
                                  .filter(|p| p.millis_since_sent(now) > 500)
                                  .map(|p| p.get_packet_seq_number())
                                  .collect::<Vec<_>>();

        println!("EXPIRED PACKETS: {:?}", expired_packets);

        for packet in expired_packets {
            self.resend_packet(packet).await?;
        }

        Ok(())
    }

    async fn wait_for_reception(&mut self) -> Result<()> {
        let last_seq = self.seq_number - 1;

        while !self.is_packet_acked(last_seq) {
            self.receive_and_handle_lost_packets().await?;
        }

        Ok(())
    }

    fn is_packet_acked(&self, n: SequenceNumber) -> bool {
        !self.inflight_packets.iter().any(|p| p.get_packet_seq_number() == n)
    }

    async fn send_packet(&mut self, mut packet: Packet) -> Result<()> {

        packet.set_ack_number(self.ack_number);
        packet.set_packet_seq_number(self.seq_number);
        packet.set_connection_id(self.send_id);
        packet.set_window_size(1_048_576);
        self.seq_number += 1;
        packet.update_timestamp();

        println!("SENDING NEW PACKET ! {:?}", packet.get_seq_number());

        // println!("SENDING {:?}", &*packet);

        self.udp.send(packet.as_bytes()).await?;

        self.inflight_packets.push_back(packet);

        Ok(())
    }

    async fn receive_packet(&mut self) -> Result<()> {
        let mut buffer = [0; 1500];

        let mut timeout = self.congestion_timeout;
        let mut len = None;

        for _ in 0..3 {
            match self.udp.recv_timeout(&mut buffer, timeout).await {
                Ok(n) => {
                    len = Some(n);
                    break;
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    timeout *= 2;
                    continue;
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        }

        if let Some(len) = len {
            let packet = PacketRef::ref_from_buffer(&buffer[..len])?;
            self.dispatch(packet).await?;
            return Ok(());
        };

        Err(Error::new(ErrorKind::TimedOut, "timed out").into())
    }

    /// Returns the number of bytes currently in flight (sent but not acked)
    fn inflight_size(&self) -> usize {
        self.inflight_packets.iter().map(Packet::size).sum()
    }

    fn packet_size(&self) -> usize {
        let is_ipv4 = self.remote.map(|r| r.is_ipv4()).unwrap_or(true);

        // TODO: Change this when MTU discovery is implemented
        if is_ipv4 {
            UDP_IPV4_MTU - HEADER_SIZE
        } else {
            UDP_IPV6_MTU - HEADER_SIZE
        }
    }

    async fn dispatch(&mut self, packet: PacketRef<'_>) -> Result<()> {
        //println!("DISPATCH HEADER: {:?}", packet.header());

        self.delay = Delay::since(packet.get_timestamp());

        match (packet.get_type()?, self.state) {
            (PacketType::Syn, State::None) => {
                self.state = State::Connected;
                // Set self.remote
                let connection_id = packet.get_connection_id();
                self.recv_id = connection_id + 1;
                self.send_id = connection_id;
                self.seq_number = SequenceNumber::random();
                self.ack_number = packet.get_seq_number();
            }
            (PacketType::Syn, _) => {
            }
            (PacketType::State, State::SynSent) => {
                self.state = State::Connected;
                // Related:
                // https://engineering.bittorrent.com/2015/08/27/drdos-udp-based-protocols-and-bittorrent/
                // https://www.usenix.org/system/files/conference/woot15/woot15-paper-adamsky.pdf
                // https://github.com/bittorrent/libutp/commit/13d33254262d46b638d35c4bc1a2f76cea885760
                self.ack_number = packet.get_seq_number() - 1;
                self.remote_window = packet.get_window_size();
                println!("CONNECTED !", );
            }
            (PacketType::State, State::Connected) => {
                if self.remote_window != packet.get_window_size() {
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

    fn on_data_loss(&mut self) {
        // on data loss:
        // # at most once per RTT
        // cwnd = min (cwnd, max (cwnd/2, MIN_CWND * MSS))
        // if data lost is not to be retransmitted:
        //     flightsize = flightsize - bytes_not_to_be_retransmitted
        let cwnd = self.cwnd;
        self.cwnd = cwnd.min((cwnd / 2).max(MIN_CWND * MSS));
        // TODO:
        // if data lost is not to be retransmitted:
        //     flightsize = flightsize - bytes_not_to_be_retransmitted
    }

    fn on_congestion_timeout_expired(&mut self) {
        // if no ACKs are received within a CTO:
        // # extreme congestion, or significant RTT change.
        // # set cwnd to 1MSS and backoff the congestion timer.
        // cwnd = 1 * MSS
        self.cwnd = MSS;
        self.congestion_timeout *= 2;
    }

    async fn handle_state(&mut self, packet: PacketRef<'_>) -> Result<()> {
        let ack_number = packet.get_ack_number();

        println!("ACK RECEIVED {:?} LAST_ACK {:?} DUP {:?} INFLIGHT {:?}",
                 ack_number, self.last_ack, self.ack_duplicate, self.inflight_size());

        let in_flight = self.inflight_size();
        let mut bytes_newly_acked = 0;

        let before = self.inflight_packets.len();
        self.inflight_packets
            .retain(|p| {
                !p.is_seq_less_equal(ack_number) || (false, bytes_newly_acked += p.size()).0
            });

        //println!("PACKETS IN FLIGHT {:?}", self.inflight_packets.len());
//        println!("PACKETS IN FLIGHT {:?}", self.inflight_packets.as_slices());

        println!("BEFORE {:?} AFTER {:?}", before, self.inflight_packets.len());

        let delay = packet.get_timestamp_diff();
        if !delay.is_zero() {
            self.delay_history.add_delay(delay);
            self.apply_congestion_control(bytes_newly_acked, in_flight);
        }

        if self.last_ack == ack_number {
            self.ack_duplicate = self.ack_duplicate.saturating_add(1);
            if self.ack_duplicate >= 3 {
                self.lost_packets.push_back(ack_number + 1);
                return Err(UtpError::PacketLost);
            }
        } else {
            self.ack_duplicate = 1;
            self.last_ack = ack_number;
        }

        if packet.has_extension() {
            println!("HAS EXTENSIONS !", );
            let mut lost = false;
            for select_ack in packet.iter_sacks() {
                lost = select_ack.has_missing_ack() || lost;
                println!("SACKS ACKED: {:?}", select_ack.nackeds());
                for ack_bit in select_ack {
                    match ack_bit {
                        SelectiveAckBit::Missing(seq_num) => {
                            self.lost_packets.push_back(seq_num);
                        }
                        SelectiveAckBit::Acked(seq_num) => {
                            self.inflight_packets
                                .retain(|p| !(p.get_packet_seq_number() == seq_num));
                        }
                    }
                }
            }
            if lost {
                println!("MISSING FROM SACK {:?}", self.lost_packets);
                return Err(UtpError::PacketLost);
            }
        }

        Ok(())
    }

    async fn resend_packet(&mut self, start: SequenceNumber) -> Result<()> {
        println!("PACKET TO RESEND {:?}", start);
        let mut packet = self.inflight_packets.iter_mut().find(|p| p.get_packet_seq_number() == start);
        if let Some(ref mut packet) = packet {

            let packet_size = packet.size();

            // if !packet.resent {
                packet.set_ack_number(self.ack_number);
                packet.update_timestamp();

                println!("== RESENDING {:?} CWND={:?} POS {:?} {:?}", start, self.cwnd, start, &**packet);

                self.udp.send(packet.as_bytes()).await?;

                packet.resent = true;
            // } else {
            //     println!("SKIPPING PACKET {:?}", packet);
            // }

        } else {
            println!("PACKET NOT FOUND", );
        }
        //return packet;

        Ok(())
    }

    fn apply_congestion_control(&mut self, bytes_newly_acked: usize, in_flight: usize) {
        let lowest_relative = self.delay_history.lowest_relative();

        let cwnd_reached = in_flight + bytes_newly_acked + self.packet_size() > self.cwnd as usize;

        if cwnd_reached {
            let window_factor = I48F16::from_num(bytes_newly_acked as i64) / in_flight as i64;
            let delay_factor = I48F16::from_num(TARGET - lowest_relative.as_i64()) / TARGET;

            let gain = (window_factor * delay_factor) * 3000;

            self.cwnd = self.remote_window.min((self.cwnd as i32 + gain.to_num::<i32>()).max(0) as u32);

            println!("!! CWND CHANGED !! {:?} WIN_FACT {:?} DELAY_FACT {:?} GAIN {:?}", self.cwnd, window_factor, delay_factor, gain);
        }
    }

    fn update_congestion_timeout(&mut self) {
        // TODO
    }

    async fn send_ack(&mut self) -> Result<()> {
        let mut header = Header::new(PacketType::State);
        header.set_connection_id(self.send_id);
        header.set_seq_number(self.seq_number);
        header.set_ack_number(self.ack_number);
        self.seq_number += 1;

        Ok(())
    }
}
