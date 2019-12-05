
use async_std::net::{UdpSocket, SocketAddr};
use async_std::io::{ErrorKind, Error};
use rand::Rng;

use std::time::{Duration, Instant};
use std::{iter::Iterator, collections::VecDeque};

use super::{ConnectionId, Result, UtpError, Packet, PacketRef, PacketType, Header, Delay, Timestamp, SequenceNumber};
use crate::udp_ext::WithTimeout;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
enum State {
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

const BASE_HISTORY: usize = 10;
const INIT_CWND: u32 = 2;
const MIN_CWND: u32 = 2;
/// Sender's Maximum Segment Size
/// Set to Ethernet MTU
const MSS: u32 = 1400;
const TARGET: u32 = 100;
const GAIN: u32 = 1;
const ALLOWED_INCREASE: u32 = 1;

pub struct UtpSocket {
    local: SocketAddr,
    remote: Option<SocketAddr>,
    udp: UdpSocket,
    recv_id: ConnectionId,
    send_id: ConnectionId,
    // seq_nr: u16,
    state: State,
    ack_number: SequenceNumber,
    seq_number: SequenceNumber,
    delay: Delay,

    /// Packets sent but we didn't receive an ack for them
    inflight_packets: VecDeque<Packet>,

    base_delays: VecDeque<Delay>,

    current_delays: VecDeque<Delay>, // TODO: Use SliceDeque ?

    last_rollover: Instant,

    flight_size: u32,

    cwnd: u32,
    congestion_timeout: Duration,

    /// SRTT (smoothed round-trip time)
    srtt: u32,
    /// RTTVAR (round-trip time variation)
    rttvar: u32,
}

impl UtpSocket {
    fn new(local: SocketAddr, udp: UdpSocket) -> UtpSocket {
        let (recv_id, send_id) = ConnectionId::make_ids();

        let mut base_delays = VecDeque::with_capacity(BASE_HISTORY);
        for _ in 0..BASE_HISTORY {
            base_delays.push_back(Delay::infinity());
        }

        UtpSocket {
            local,
            udp,
            recv_id,
            send_id,
            base_delays,
            remote: None,
            state: State::None,
            ack_number: SequenceNumber::zero(),
            seq_number: SequenceNumber::random(),
            delay: Delay::default(),
            current_delays: VecDeque::with_capacity(16),
            last_rollover: Instant::now(),
            cwnd: INIT_CWND * MSS,
            congestion_timeout: Duration::from_secs(1),
            flight_size: 0,
            srtt: 0,
            rttvar: 0,
            inflight_packets: VecDeque::with_capacity(64),
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

        let mut header = Header::new(PacketType::Syn);
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

    pub async fn send(&mut self, data: &[u8]) -> Result<usize> {
        let mut packet = Packet::new(&data);
        let mut seq_number = self.seq_number;
        packet.set_ack_number(self.ack_number);
        packet.set_connection_id(self.send_id);
        packet.set_window_size(1_048_576);

        let mut len = None;
        let mut buffer = [0; 1500];

        for _ in 0..3 {
            packet.update_timestamp();
            packet.set_seq_number(seq_number);
            seq_number += 1;
            println!("SENDING {:#?}", &(&*packet));

            // println!("SENDING BUFFER", );
            self.udp.send(packet.as_bytes()).await?;

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

        println!("RECEIVED {:?}", len);
        if let Some(len) = len {
            let packet = PacketRef::ref_from_buffer(&buffer[..len])?;
            self.dispatch(packet).await?;
            return Ok(len);
        }

        Err(Error::new(ErrorKind::TimedOut, "Send timed out").into())
    }

    async fn dispatch(&mut self, packet: PacketRef<'_>) -> Result<()> {
        println!("DISPATCH HEADER: {:#?}", packet.header());

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

                println!("CONNECTED !", );
            }
            (PacketType::State, State::Connected) => {
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

    fn update_base_delay(&mut self, delay: Delay) {
        // # Maintain BASE_HISTORY delay-minima.
        // # Each minimum is measured over a period of a minute.
        // # 'now' is the current system time
        // if round_to_minute(now) != round_to_minute(last_rollover)
        //     last_rollover = now
        //     delete first item in base_delays list
        //     append delay to base_delays list
        // else
        //     base_delays.tail = MIN(base_delays.tail, delay)
        if self.last_rollover.elapsed() >= Duration::from_secs(1) {
            self.last_rollover = Instant::now();
            self.base_delays.pop_front();
            self.base_delays.push_back(delay);
        } else {
            let last = self.base_delays.pop_back().unwrap();
            self.base_delays.push_back(last.min(delay));
        }
    }

    fn update_current_delay(&mut self, delay: Delay) {
        //  # Maintain a list of CURRENT_FILTER last delays observed.
        // delete first item in current_delays list
        // append delay to current_delays list

        // TODO: Pop delays before the last RTT
        self.current_delays.pop_front();
        self.current_delays.push_back(delay);
    }

    fn filter_current_delays(&self) -> Delay {
        // TODO: Test other algos

        // We're using the exponentially weighted moving average (EWMA) function
        // Magic number from https://github.com/VividCortex/ewma
        let alpha = 0.032_786_885;
        let mut samples = self.current_delays.iter().map(|d| d.as_num() as f64);
        let first = samples.next().unwrap_or(0.0);
        (samples.fold(
            first,
            |acc, delay| alpha * delay + (acc * (1.0 - alpha))
        ) as i64).into()
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

    fn handle_state(&mut self, packet: &PacketRef<'_>) {
        let ack_number = packet.get_ack_number();
        let ack_packet = self.inflight_packets.iter().find(|p| p.get_seq_number() == ack_number);
        let ack_packets = self.inflight_packets.iter().filter(|p| p.get_seq_number().cmp_less_equal(ack_number));
    }

    fn handle_ack(&mut self, packet: &PacketRef<'_>) {
        // flightsize is the amount of data outstanding before this ACK
        //    was received and is updated later;
        // bytes_newly_acked is the number of bytes that this ACK
        //    newly acknowledges, and it MAY be set to MSS.

        let delay = packet.get_timestamp_diff();
        self.update_base_delay(delay);
        self.update_current_delay(delay);

        let queuing_delay = self.filter_current_delays()
            - *self.base_delays.iter().min().unwrap();
        let queuing_delay: u32 = queuing_delay.into();

        let off_target = (TARGET - queuing_delay) / TARGET;

        // TODO: Compute bytes_newly_acked;
        let bytes_newly_acked = 10;

        let cwnd = GAIN * off_target * bytes_newly_acked * MSS / self.cwnd;
        let max_allowed_cwnd = self.flight_size + (ALLOWED_INCREASE * MSS);

        let cwnd = cwnd.min(max_allowed_cwnd);

        self.cwnd = cwnd.max(MIN_CWND * MSS);
        self.flight_size -= bytes_newly_acked;

//        let cwnd = std::cmp::min(cwnd, max_allowed_cwnd);

       // for each delay sample in the acknowledgement:
       //     delay = acknowledgement.delay
       //     update_base_delay(delay)
       //     update_current_delay(delay)

       // queuing_delay = FILTER(current_delays) - MIN(base_delays)
       // off_target = (TARGET - queuing_delay) / TARGET
       // cwnd += GAIN * off_target * bytes_newly_acked * MSS / cwnd
       // max_allowed_cwnd = flightsize + ALLOWED_INCREASE * MSS
       // cwnd = min(cwnd, max_allowed_cwnd)
       // cwnd = max(cwnd, MIN_CWND * MSS)
       // flightsize = flightsize - bytes_newly_acked
       // update_CTO()
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
