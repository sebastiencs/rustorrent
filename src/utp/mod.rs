use std::{
    cmp::{Ord, PartialOrd},
    io::ErrorKind,
    mem::MaybeUninit,
    ops::{Add, AddAssign, Deref, DerefMut, Sub, SubAssign},
};

mod ack_bitfield;
pub mod stream;
pub mod tick;
//pub mod writer;
pub mod listener;
pub mod manager;
pub mod udp_socket;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum UtpState {
    /// not yet connected
    None,
    /// Syn received, waiting for data
    SynReceived,
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
    MustConnect, // /// ====== states beyond this point =====
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

impl From<u8> for UtpState {
    fn from(n: u8) -> UtpState {
        match n {
            0 => UtpState::None,
            1 => UtpState::SynReceived,
            2 => UtpState::SynSent,
            3 => UtpState::Connected,
            4 => UtpState::FinSent,
            5 => UtpState::MustConnect,
            _ => unreachable!(),
        }
    }
}

impl From<UtpState> for u8 {
    fn from(s: UtpState) -> u8 {
        match s {
            UtpState::None => 0,
            UtpState::SynReceived => 1,
            UtpState::SynSent => 2,
            UtpState::Connected => 3,
            UtpState::FinSent => 4,
            UtpState::MustConnect => 5,
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

/// A safe type using wrapping_{add,sub} for +/-/cmp operations
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct SequenceNumber(u16);

impl SequenceNumber {
    pub fn random() -> SequenceNumber {
        SequenceNumber(rand::thread_rng().gen())
    }

    pub fn zero() -> SequenceNumber {
        SequenceNumber(0)
    }

    pub(self) fn to_be(self) -> u16 {
        u16::to_be(self.0)
    }

    pub(self) fn from_be(n: u16) -> SequenceNumber {
        SequenceNumber(u16::from_be(n))
    }

    /// Compare self with other, with consideration to wrapping.
    /// We can't implement PartialOrd because it doesn't satisfy
    /// antisymmetry
    pub fn cmp_less(self, other: SequenceNumber) -> bool {
        let dist_down = self - other;
        let dist_up = other - self;

        dist_up.0 < dist_down.0
    }

    /// Compare self with other, with consideration to wrapping.
    /// We can't implement PartialOrd because it doesn't satisfy
    /// antisymmetry
    pub fn cmp_less_equal(self, other: SequenceNumber) -> bool {
        let dist_down = self - other;
        let dist_up = other - self;

        dist_up.0 <= dist_down.0
    }

    /// Compare self with other, with consideration to wrapping.
    /// We can't implement PartialOrd because it doesn't satisfy
    /// antisymmetry
    pub fn cmp_greater(self, other: SequenceNumber) -> bool {
        !self.cmp_less_equal(other)
    }
}

impl From<u16> for SequenceNumber {
    fn from(n: u16) -> SequenceNumber {
        SequenceNumber(n)
    }
}

impl From<SequenceNumber> for u16 {
    fn from(s: SequenceNumber) -> u16 {
        s.0
    }
}

impl Add<u16> for SequenceNumber {
    type Output = Self;

    fn add(self, n: u16) -> Self {
        Self(self.0.wrapping_add(n))
    }
}
impl AddAssign<u16> for SequenceNumber {
    fn add_assign(&mut self, other: u16) {
        // Use Add impl, with wrapping
        *self = *self + other;
    }
}

impl Sub<u16> for SequenceNumber {
    type Output = Self;

    fn sub(self, n: u16) -> Self {
        Self(self.0.wrapping_sub(n))
    }
}
impl Sub<SequenceNumber> for SequenceNumber {
    type Output = Self;

    fn sub(self, n: SequenceNumber) -> Self {
        Self(self.0.wrapping_sub(n.0))
    }
}
impl SubAssign<u16> for SequenceNumber {
    fn sub_assign(&mut self, other: u16) {
        // Use Sub impl, with wrapping
        *self = *self - other;
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Timestamp(u32);

impl Timestamp {
    pub fn now() -> Timestamp {
        use crate::time;

        let (sec, nano) = time::get_time();
        Timestamp((sec * 1_000_000 + nano / 1000) as u32)
        // let since_epoch = coarsetime::Clock::now_since_epoch();
        // println!("SINCE_EPOCH {:?}", since_epoch);
        // let now = since_epoch.as_secs() * 1_000_000 + (since_epoch.subsec_nanos() / 1000) as u64;
        // Timestamp(now as u32)
    }

    pub(self) fn zero() -> Timestamp {
        Timestamp(0)
    }

    pub fn elapsed_millis(self, now: Timestamp) -> u32 {
        //let now = Timestamp::now().0 / 1000;
        (now.0 / 1000).saturating_sub(self.0 / 1000)
    }

    // pub fn elapsed_millis(self) -> u32 {
    //     let now = Timestamp::now().0 / 1000;
    //     now - (self.0 / 1000)
    // }

    // return uint64(ts.tv_sec) * 1000000 + uint64(ts.tv_nsec) / 1000;

    pub fn delay(self, o: Timestamp) -> Delay {
        if self.0 > o.0 {
            (self.0 - o.0).into()
        } else {
            (o.0 - self.0).into()
        }
    }
}

impl From<u32> for Timestamp {
    fn from(n: u32) -> Timestamp {
        Timestamp(n)
    }
}

impl Into<u32> for Timestamp {
    fn into(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Copy, Clone, Default, Ord, PartialEq, Eq, PartialOrd)]
pub struct RelativeDelay(u32);

impl RelativeDelay {
    fn infinity() -> RelativeDelay {
        RelativeDelay(u32::max_value())
    }

    pub fn as_i64(self) -> i64 {
        self.0 as i64
    }
}

#[derive(Debug, Copy, Clone, Default, Ord, PartialEq, Eq, PartialOrd)]
//#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct Delay(u32);

impl Delay {
    pub fn since(timestamp: Timestamp) -> Delay {
        let now = Timestamp::now();
        now.delay(timestamp)
    }

    pub fn infinity() -> Delay {
        Delay(u32::max_value())
    }

    // /// Compare self with other, with consideration to wrapping.
    // /// We can't implement PartialOrd because it doesn't satisfy
    // /// antisymmetry
    // pub fn cmp_less(self, other: Delay) -> bool {
    //     let dist_down = self - other;
    //     let dist_up = other - self;

    //     println!("DIST_DOWN {:?} DIST_UP {:?}", dist_down, dist_up);
    //     dist_up.0 < dist_down.0
    // }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl Sub for Delay {
    type Output = RelativeDelay;

    fn sub(self, other: Delay) -> RelativeDelay {
        RelativeDelay(self.0.saturating_sub(other.0))
    }
}

impl From<u32> for Delay {
    fn from(n: u32) -> Delay {
        Delay(n)
    }
}

// impl From<i64> for Delay {
//     fn from(n: i64) -> Delay {
//         Delay(n)
//     }
// }

impl Into<u32> for Delay {
    fn into(self) -> u32 {
        self.0 as u32
    }
}

use std::time::{Duration, Instant};

#[derive(Debug)]
struct DelayHistory {
    /// Array of delays, 1 per minute
    /// history[x] is the lowest delay in the minute x
    history: [Delay; 20],
    /// Index in `history` array
    index: u8,
    /// Number of delays in the current minute
    ndelays: u16,
    /// Lowest delay in the last 20 mins
    lowest: Delay,
    next_index_time: Instant,
    /// 3 lowest relative delays
    last_relatives: [RelativeDelay; 3],
    /// Index in `last_relatives`
    index_relative: u8,
}

impl DelayHistory {
    fn new() -> DelayHistory {
        DelayHistory {
            history: [Delay::infinity(); 20],
            index: 0,
            ndelays: 0,
            lowest: Delay::infinity(),
            next_index_time: Instant::now() + Duration::from_secs(1),
            last_relatives: [RelativeDelay::infinity(); 3],
            index_relative: 0,
        }
    }

    pub fn get_lowest(&self) -> Delay {
        self.lowest
    }

    pub fn add_delay(&mut self, delay: Delay) {
        self.ndelays = self.ndelays.saturating_add(1);

        let index = self.index as usize;
        if delay < self.lowest {
            self.lowest = delay;
            self.history[index] = delay;
        } else if delay < self.history[index] {
            self.history[index] = delay;
        }

        let value = delay - self.lowest;
        self.save_relative(value);

        if self.ndelays > 120
            && self
                .next_index_time
                .checked_duration_since(Instant::now())
                .is_some()
        {
            self.next_index_time = Instant::now() + Duration::from_secs(1);
            self.index = (self.index + 1) % self.history.len() as u8;
            self.ndelays = 0;
            self.history[self.index as usize] = delay;
            self.lowest = self.history.iter().min().copied().unwrap();
        }

        //println!("HISTORY {:?}", self);
        //println!("VALUE {:?} FROM {:?} AND {:?}", value, delay, self.lowest);
    }

    fn save_relative(&mut self, relative: RelativeDelay) {
        let index = self.index_relative as usize;
        self.last_relatives[index] = relative;
        self.index_relative = ((index + 1) % self.last_relatives.len()) as u8;
    }

    fn lowest_relative(&self) -> RelativeDelay {
        self.last_relatives.iter().min().copied().unwrap()
    }

    // fn lowest_in_history(&self) -> Delay {
    //     let mut lowest = self.history[0];
    //     for delay in &self.history {
    //         if delay.cmp_less(lowest) {
    //             lowest = *delay;
    //         }
    //     }
    //     lowest
    // }
}

#[derive(Debug)]
pub enum UtpError {
    Malformed,
    UnknownPacketType,
    WrongVersion,
    FamillyMismatch,
    PacketLost,
    MustClose,
    PacketNotSent,
    IO(std::io::Error),
    RecvError(async_channel::RecvError),
    RecvClosed,
    SendWouldBlock,
}

impl UtpError {
    pub fn should_continue(&self) -> bool {
        match self {
            UtpError::IO(ref e)
                if e.kind() == ErrorKind::TimedOut || e.kind() == ErrorKind::WouldBlock =>
            {
                true
            }
            _ => false,
        }
    }
}

impl From<std::io::Error> for UtpError {
    fn from(e: std::io::Error) -> UtpError {
        UtpError::IO(e)
    }
}

type Result<T> = std::result::Result<T, UtpError>;

#[derive(Debug, PartialEq, Eq)]
pub enum PacketType {
    /// regular data packet. Socket is in connected state and has
    /// data to send. An ST_DATA packet always has a data payload.
    Data,
    /// Finalize the connection. This is the last packet. It closes the
    /// connection, similar to TCP FIN flag. This connection will
    /// never have a sequence number greater than the sequence number
    /// in this packet. The socket records this sequence number
    /// as eof_pkt. This lets the socket wait for packets that might
    /// still be missing and arrive out of order even after receiving the ST_FIN packet.
    Fin,
    /// State packet. Used to transmit an ACK with no data. Packets
    /// that don't include any payload do not increase the seq_nr.
    State,
    /// Terminate connection forcefully. Similar to TCP RST flag.
    /// The remote host does not have any state for this
    /// connection. It is stale and should be terminated.
    Reset,
    /// Connect SYN. Similar to TCP SYN flag, this packet initiates a
    /// connection. The sequence number is initialized to 1. The connection
    /// ID is initialized to a random number. The syn packet is special,
    /// all subsequent packets sent on this connection (except for re-sends
    /// of the ST_SYN) are sent with the connection ID + 1. The connection
    /// ID is what the other end is expected to use in its responses.
    /// When receiving an ST_SYN, the new socket should be initialized with
    /// the ID in the packet header. The send ID for the socket should
    /// be initialized to the ID + 1. The sequence number for the return
    /// channel is initialized to a random number. The other end expects an
    /// ST_STATE packet (only an ACK) in response.
    Syn,
}

use std::convert::{TryFrom, TryInto};

impl TryFrom<u8> for PacketType {
    type Error = UtpError;

    fn try_from(type_version: u8) -> Result<PacketType> {
        let packet_type = type_version >> 4;
        match packet_type {
            0 => Ok(PacketType::Data),
            1 => Ok(PacketType::Fin),
            2 => Ok(PacketType::State),
            3 => Ok(PacketType::Reset),
            4 => Ok(PacketType::Syn),
            _ => Err(UtpError::UnknownPacketType),
        }
    }
}

impl Into<u8> for PacketType {
    fn into(self) -> u8 {
        match self {
            PacketType::Data => 0,
            PacketType::Fin => 1,
            PacketType::State => 2,
            PacketType::Reset => 3,
            PacketType::Syn => 4,
        }
    }
}

pub const HEADER_SIZE: usize = std::mem::size_of::<Header>();

/// 0       4       8               16              24              32
/// +-------+-------+---------------+---------------+---------------+
/// | type  | ver   | extension     | connection_id                 |
/// +-------+-------+---------------+---------------+---------------+
/// | timestamp_microseconds                                        |
/// +---------------+---------------+---------------+---------------+
/// | timestamp_difference_microseconds                             |
/// +---------------+---------------+---------------+---------------+
/// | wnd_size                                                      |
/// +---------------+---------------+---------------+---------------+
/// | seq_nr                        | ack_nr                        |
/// +---------------+---------------+---------------+---------------+

#[repr(C, packed)]
pub struct Header {
    /// Protocol version and packet type.
    /// type = type_version >> 4
    /// version = type_version & 0x0F
    /// The current version is 1.
    /// The type field describes the type of packet.
    type_version: u8,
    /// The type of the first extension in a linked list of extension headers.
    /// 0 means no extension.
    extension: u8,
    /// This is a random, unique, number identifying all the packets
    /// that belong to the same connection. Each socket has one
    /// connection ID for sending packets and a different connection
    /// ID for receiving packets. The endpoint initiating the connection
    /// decides which ID to use, and the return path has the same ID + 1.
    connection_id: u16,
    /// This is the 'microseconds' parts of the timestamp of when this packet
    /// was sent. This is set using gettimeofday() on posix and
    /// QueryPerformanceTimer() on windows. The higher resolution this timestamp
    /// has, the better. The closer to the actual transmit time it is set, the better.
    timestamp_micro: u32,
    /// This is the difference between the local time and the timestamp in the last
    /// received packet, at the time the last packet was received. This is the
    /// latest one-way delay measurement of the link from the remote peer to the local
    /// machine. When a socket is newly opened and doesn't have any delay
    /// samples yet, this must be set to 0.
    timestamp_difference_micro: u32,
    /// Advertised receive window. This is 32 bits wide and specified in bytes.
    /// The window size is the number of bytes currently in-flight, i.e. sent but
    /// not acked. The advertised receive window lets the other end cap the
    /// window size if it cannot receive any faster, if its receive buffer is
    /// filling up.
    /// When sending packets, this should be set to the number of bytes left in
    /// the socket's receive buffer.
    window_size: u32,
    /// This is the sequence number of this packet. As opposed to TCP, uTP
    /// sequence numbers are not referring to bytes, but packets. The sequence
    /// number tells the other end in which order packets should be served back
    /// to the application layer.
    seq_nr: u16,
    /// This is the sequence number the sender of the packet last received in the
    /// other direction.
    ack_nr: u16,
}

impl Header {
    fn get_type(&self) -> Result<PacketType> {
        self.check_version()?;
        self.type_version.try_into()
    }

    fn check_version(&self) -> Result<()> {
        match self.type_version & 0x0F {
            1 => Ok(()),
            _ => Err(UtpError::WrongVersion),
        }
    }

    // Getters
    fn get_connection_id(&self) -> ConnectionId {
        u16::from_be(self.connection_id).into()
    }
    fn get_version(&self) -> u8 {
        self.type_version & 0xF
    }
    fn get_timestamp(&self) -> Timestamp {
        u32::from_be(self.timestamp_micro).into()
    }
    fn get_timestamp_diff(&self) -> Delay {
        u32::from_be(self.timestamp_difference_micro).into()
    }
    fn get_window_size(&self) -> u32 {
        u32::from_be(self.window_size)
    }
    fn get_seq_number(&self) -> SequenceNumber {
        SequenceNumber::from_be(self.seq_nr)
    }
    fn get_ack_number(&self) -> SequenceNumber {
        SequenceNumber::from_be(self.ack_nr)
    }
    fn get_extension_type(&self) -> ExtensionType {
        ExtensionType::from(self.extension)
    }
    fn has_extension(&self) -> bool {
        self.extension != 0
    }

    // Setters
    fn set_connection_id(&mut self, id: ConnectionId) {
        self.connection_id = u16::to_be(id.into());
    }
    fn set_timestamp(&mut self, timestamp: Timestamp) {
        self.timestamp_micro = u32::to_be(timestamp.into());
    }
    fn set_timestamp_diff(&mut self, delay: Delay) {
        self.timestamp_difference_micro = u32::to_be(delay.into());
    }
    fn set_window_size(&mut self, window_size: u32) {
        self.window_size = u32::to_be(window_size);
    }
    fn set_seq_number(&mut self, seq_number: SequenceNumber) {
        self.seq_nr = seq_number.to_be();
    }
    fn set_ack_number(&mut self, ack_number: SequenceNumber) {
        self.ack_nr = ack_number.to_be();
    }

    // fn update_timestamp(&mut self) {
    //     self.set_timestamp(Timestamp::now());
    // }

    pub fn new(packet_type: PacketType) -> Header {
        let packet_type: u8 = packet_type.into();
        Header {
            type_version: packet_type << 4 | 1,
            ..Default::default()
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe { &*(self as *const Header as *const [u8; std::mem::size_of::<Header>()]) }
    }
}

impl Default for Header {
    fn default() -> Header {
        let packet_type: u8 = PacketType::Data.into();
        Header {
            type_version: packet_type << 4 | 1,
            extension: 0,
            connection_id: 0,
            timestamp_micro: 0,
            timestamp_difference_micro: 0,
            window_size: 0,
            seq_nr: 0,
            ack_nr: 0,
        }
    }
}

impl std::fmt::Debug for Header {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Header")
            .field("type", &self.get_type())
            .field("version", &self.get_version())
            // .field("extension", &self.get_extension_type())
            .field("connection_id", &self.get_connection_id())
            .field("timestamp", &self.get_timestamp())
            .field("timestamp_difference", &self.get_timestamp_diff())
            .field("window_size", &self.get_window_size())
            .field("seq_number", &self.get_seq_number())
            .field("ack_number", &self.get_ack_number())
            .finish()
    }
}

#[derive(Copy, Clone, Debug)]
struct ConnectionId(u16);

impl From<u16> for ConnectionId {
    fn from(byte: u16) -> ConnectionId {
        ConnectionId(byte)
    }
}

impl Into<u16> for ConnectionId {
    fn into(self) -> u16 {
        self.0
    }
}

impl Add<u16> for ConnectionId {
    type Output = Self;

    fn add(self, o: u16) -> ConnectionId {
        ConnectionId(self.0 + o)
    }
}

use rand::Rng;

impl ConnectionId {
    pub fn make_ids() -> (ConnectionId, ConnectionId) {
        let id = rand::thread_rng().gen::<u16>();
        if id == 0 {
            (id.into(), (id + 1).into())
        } else {
            ((id - 1).into(), id.into())
        }
    }
}

const PAYLOAD_SIZE: usize = 1500;

#[repr(C, packed)]
struct Payload {
    data: [u8; PAYLOAD_SIZE],
    len: usize,
}

impl Payload {
    fn new_in_place(place: &mut Payload, data: &[u8]) {
        let data_len = data.len();
        place.data[..data_len].copy_from_slice(data);
        place.len = data_len;
    }

    fn new(data: &[u8]) -> Payload {
        let data_len = data.len();
        let mut payload = [0; PAYLOAD_SIZE];
        payload[..data_len].copy_from_slice(data);
        Payload {
            data: payload,
            len: data_len,
        }
    }

    pub fn append(&mut self, data: &[u8]) {
        let data_len = data.len();

        assert!(data_len > 0);

        self.data[..data_len].copy_from_slice(data);
        self.len += data_len;
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

pub struct PacketPool {
    pool: Vec<Packet>,
}

const PACKET_MAX_SIZE: usize = HEADER_SIZE + PAYLOAD_SIZE;

#[repr(C, packed)]
pub struct Packet {
    header: Header,
    payload: Payload,
    /// Used to read the seq_nr later, without the need to convert from
    /// big endian from the header
    seq_number: SequenceNumber,
    /// True if this packet was resent
    resent: bool,
    last_sent: Timestamp,
    lost: bool,
    received_at: Option<Timestamp>,
}

impl std::fmt::Debug for Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let seq_number = self.seq_number;
        f.debug_struct("Packet")
            .field("seq_nr", &seq_number)
            .field("header", &self.header)
            .finish()
    }
}

impl Deref for Packet {
    type Target = Header;

    fn deref(&self) -> &Header {
        &self.header
    }
}

impl DerefMut for Packet {
    //type Target = Header;

    fn deref_mut(&mut self) -> &mut Header {
        &mut self.header
    }
}

impl Packet {
    pub fn new_in_place<'a>(place: &'a mut MaybeUninit<Packet>, data: &[u8]) -> &'a Packet {
        let place = unsafe { &mut *place.as_mut_ptr() };

        place.header = Header::default();
        Payload::new_in_place(&mut place.payload, data);
        // Fill rest of Packet with non-uninitialized data
        // Ensure that we don't invoke any Drop here
        place.seq_number = SequenceNumber::zero();
        place.resent = false;
        place.last_sent = Timestamp::zero();
        place.lost = false;
        place.received_at = None;

        place
    }

    //    pub fn from_incoming_in_place(place: &mut Packet, data: &[u8], timestamp: Timestamp) {
    pub fn from_incoming_in_place<'a>(
        place: &'a mut MaybeUninit<Packet>,
        data: &[u8],
        timestamp: Timestamp,
    ) -> &'a Packet {
        //let slice = unsafe { &mut *(place as *mut Packet as *mut [u8; PACKET_MAX_SIZE]) };
        let slice = unsafe { &mut *(place.as_mut_ptr() as *mut [u8; PACKET_MAX_SIZE]) };
        let data_len = data.len();

        assert!((HEADER_SIZE..PACKET_MAX_SIZE).contains(&data_len));

        slice[..data_len].copy_from_slice(data);

        let place = unsafe { &mut *place.as_mut_ptr() };

        // Fill rest of Packet with non-uninitialized data
        // Ensure that we don't invoke any Drop here
        place.payload.len = data_len - HEADER_SIZE;
        place.seq_number = place.get_seq_number();
        place.resent = false;
        place.last_sent = Timestamp::zero();
        place.lost = false;
        place.received_at = Some(timestamp);

        place
    }

    pub fn new(data: &[u8]) -> Packet {
        Packet {
            header: Header::default(),
            payload: Payload::new(data),
            seq_number: SequenceNumber::zero(),
            resent: false,
            last_sent: Timestamp::zero(),
            lost: false,
            received_at: None,
        }
    }

    pub fn syn() -> Packet {
        Packet {
            header: Header::new(PacketType::Syn),
            payload: Payload::new(&[]),
            seq_number: SequenceNumber::zero(),
            resent: false,
            last_sent: Timestamp::zero(),
            lost: false,
            received_at: None,
        }
    }

    pub fn new_type(ty: PacketType) -> Packet {
        Packet {
            header: Header::new(ty),
            payload: Payload::new(&[]),
            seq_number: SequenceNumber::zero(),
            resent: false,
            last_sent: Timestamp::zero(),
            lost: false,
            received_at: None,
        }
    }

    pub fn add_selective_acks(&mut self, bytes: &[u8]) {
        self.header.extension = 1;

        // TODO: Make chunks of the extention
        self.payload.append(&[0, bytes.len().try_into().unwrap()]);
        self.payload.append(bytes);
    }

    pub fn received_at(&self) -> Timestamp {
        self.received_at.expect("Packet wasn't received")
    }

    pub fn payload_len(&self) -> usize {
        self.payload.len()
    }

    pub fn iter_sacks(&self) -> ExtensionIterator {
        ExtensionIterator::new(self)
    }

    pub fn update_timestamp(&mut self) {
        let timestamp = Timestamp::now();
        self.set_timestamp(timestamp);
        self.last_sent = timestamp;
    }

    pub fn millis_since_sent(&self, now: Timestamp) -> u32 {
        self.last_sent.elapsed_millis(now)
    }

    pub fn set_packet_seq_number(&mut self, n: SequenceNumber) {
        self.header.set_seq_number(n);
        self.seq_number = n;
    }

    pub fn get_packet_seq_number(&self) -> SequenceNumber {
        self.seq_number
    }

    pub fn is_seq_less_equal(&self, n: SequenceNumber) -> bool {
        self.seq_number.cmp_less_equal(n)
    }

    pub fn size(&self) -> usize {
        self.payload.len() + HEADER_SIZE
    }

    pub fn as_bytes(&self) -> &[u8] {
        let slice =
            unsafe { &*(self as *const Packet as *const [u8; std::mem::size_of::<Packet>()]) };
        &slice[..std::mem::size_of::<Header>() + self.payload.len]
    }

    pub fn get_data(&self) -> &[u8] {
        &self.payload.data[..self.payload.len()]
    }

    // pub fn iter_extensions(&self) -> ExtensionIterator {
    //     ExtensionIterator::new(self)
    // }
}

pub enum ExtensionType {
    SelectiveAck,
    None,
    Unknown,
}

impl From<u8> for ExtensionType {
    fn from(byte: u8) -> ExtensionType {
        match byte {
            0 => ExtensionType::None,
            1 => ExtensionType::SelectiveAck,
            _ => ExtensionType::Unknown,
        }
    }
}

pub struct SelectiveAck<'a> {
    bitfield: &'a [u8],
    byte_index: usize,
    bit_index: u8,
    ack_number: SequenceNumber,
    first: bool,
}

impl SelectiveAck<'_> {
    pub fn has_missing_ack(&self) -> bool {
        self.bitfield.iter().any(|b| b.count_zeros() > 0)
    }

    pub fn nackeds(&self) -> u32 {
        self.bitfield.iter().map(|b| b.count_ones()).sum()
    }
}

pub enum SelectiveAckBit {
    Acked(SequenceNumber),
    Missing(SequenceNumber),
}

impl Iterator for SelectiveAck<'_> {
    type Item = SelectiveAckBit;

    fn next(&mut self) -> Option<Self::Item> {
        if self.first {
            self.first = false;
            return Some(SelectiveAckBit::Missing(self.ack_number + 1));
        }

        let byte = *self.bitfield.get(self.byte_index)?;
        let bit = byte & (1 << self.bit_index);

        let ack_number = self.ack_number + self.byte_index as u16 * 8 + self.bit_index as u16 + 2;

        if self.bit_index == 7 {
            self.byte_index += 1;
            self.bit_index = 0;
        } else {
            self.bit_index += 1;
        }

        if bit == 0 {
            Some(SelectiveAckBit::Missing(ack_number))
        } else {
            Some(SelectiveAckBit::Acked(ack_number))
        }
    }
}

pub struct ExtensionIterator<'a> {
    current_type: ExtensionType,
    slice: &'a [u8],
    ack_number: SequenceNumber,
}

impl<'a> ExtensionIterator<'a> {
    pub fn new(packet: &'a Packet) -> ExtensionIterator<'a> {
        let current_type = packet.get_extension_type();
        let slice = &packet.payload.data[..packet.size() - HEADER_SIZE];
        let ack_number = packet.get_ack_number();

        // for byte in &packet.packet_ref.payload.data[..packet.len - HEADER_SIZE] {
        //     //println!("BYTE {:x}", byte);
        // }
        ExtensionIterator {
            current_type,
            slice,
            ack_number,
        }
    }
}

impl<'a> Iterator for ExtensionIterator<'a> {
    type Item = SelectiveAck<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.current_type {
                ExtensionType::None => {
                    return None;
                }
                ExtensionType::SelectiveAck => {
                    let len = self.slice.get(1).copied()? as usize;
                    let bitfield = &self.slice.get(2..2 + len)?;

                    self.current_type = self.slice.get(0).copied()?.into();
                    self.slice = &self.slice.get(2 + len..)?;

                    return Some(SelectiveAck {
                        bitfield,
                        byte_index: 0,
                        bit_index: 0,
                        ack_number: self.ack_number,
                        first: true,
                    });
                }
                _ => {
                    self.current_type = self.slice.get(0).copied()?.into();
                    let len = self.slice.get(1).copied()? as usize;
                    self.slice = &self.slice.get(len..)?;
                }
            }
        }
    }
}

// Following constants found in libutp

pub const ETHERNET_MTU: usize = 1500;
pub const IPV4_HEADER_SIZE: usize = 20;
pub const IPV6_HEADER_SIZE: usize = 40;
pub const UDP_HEADER_SIZE: usize = 8;
pub const GRE_HEADER_SIZE: usize = 24;
pub const PPPOE_HEADER_SIZE: usize = 8;
pub const MPPE_HEADER_SIZE: usize = 2;
// packets have been observed in the wild that were fragmented
// with a payload of 1416 for the first fragment
// There are reports of routers that have MTU sizes as small as 1392
pub const FUDGE_HEADER_SIZE: usize = 36;
pub const TEREDO_MTU: usize = 1280;

pub const UDP_IPV4_OVERHEAD: usize = IPV4_HEADER_SIZE + UDP_HEADER_SIZE;
pub const UDP_IPV6_OVERHEAD: usize = IPV6_HEADER_SIZE + UDP_HEADER_SIZE;
pub const UDP_TEREDO_OVERHEAD: usize = UDP_IPV4_OVERHEAD + UDP_IPV6_OVERHEAD;

pub const UDP_IPV4_MTU: usize = ETHERNET_MTU
    - IPV4_HEADER_SIZE
    - UDP_HEADER_SIZE
    - GRE_HEADER_SIZE
    - PPPOE_HEADER_SIZE
    - MPPE_HEADER_SIZE
    - FUDGE_HEADER_SIZE;

pub const UDP_IPV6_MTU: usize = ETHERNET_MTU
    - IPV6_HEADER_SIZE
    - UDP_HEADER_SIZE
    - GRE_HEADER_SIZE
    - PPPOE_HEADER_SIZE
    - MPPE_HEADER_SIZE
    - FUDGE_HEADER_SIZE;

pub const UDP_TEREDO_MTU: usize = TEREDO_MTU - IPV6_HEADER_SIZE - UDP_HEADER_SIZE;

#[cfg(test)]
mod tests {
    use super::{listener::UtpListener, SequenceNumber};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn seq_number_less_equal() {
        for num in (0..u16::MAX as isize + 1024) {
            for offset in 0..1024 {
                let num_less: u16 = ((num - offset) & 0xFFFF) as u16;
                let num: u16 = (num & 0xFFFF) as u16;
                assert!(SequenceNumber::from(num_less).cmp_less_equal(SequenceNumber::from(num)));
            }
        }
    }

    #[test]
    fn seq_number_less() {
        for num in (0..u16::MAX as isize + 1024) {
            for offset in 0..1024 {
                let num_less: u16 = ((num - offset) & 0xFFFF) as u16;
                let num: u16 = (num & 0xFFFF) as u16;
                assert_eq!(
                    SequenceNumber::from(num_less).cmp_less(SequenceNumber::from(num)),
                    // nums are equals when offset = 0
                    offset != 0
                );
            }
        }
    }

    #[test]
    fn seq_number_add() {
        let seq_num = SequenceNumber::from(0);
        for add in 0..u16::MAX as usize * 3 {
            assert_eq!((seq_num + (add & 0xFFFF) as u16).0, (add & 0xFFFF) as u16);
        }
    }

    // #[test]
    // fn seq_number_sub() {
    //     let seq_num = SequenceNumber::from(0);
    //     for add in 0..u16::MAX as usize * 3 {
    //         assert_eq!(
    //             (seq_num - (add & 0xFFFF) as u16).0,
    //             (add & 0xFFFF) as u16
    //         );
    //     }
    // }

    #[test]
    fn seq_number_distance() {
        let seq_num = SequenceNumber::from(u16::MAX - 10);
        let seq_num2 = SequenceNumber::from(5);
        assert_eq!(seq_num2 - seq_num, SequenceNumber::from(16));

        let seq_num = SequenceNumber::from(2);
        let seq_num2 = SequenceNumber::from(5);
        assert_eq!(seq_num2 - seq_num, SequenceNumber::from(3));

        let seq_num = SequenceNumber::from(2);
        let seq_num2 = SequenceNumber::from(3);
        assert_eq!(seq_num2 - seq_num, SequenceNumber::from(1));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 10)]
    async fn send_data() {
        // let file = "/home/sebastien/Downloads/Escape.Plan.2013.1080p.BluRay.x265-RARBG-[rarbg.to].torrent";
        let file = "/home/sebastien/Downloads/Fedora-Cinnamon-Live-x86_64-32-1.6.iso";
        let buffer = std::fs::read(file).unwrap();

        println!("buffer length={}", buffer.len());

        let start = std::time::Instant::now();

        let listener = UtpListener::bind(std::net::SocketAddrV4::new(
            Ipv4Addr::new(0, 0, 0, 0),
            10001,
        ))
        .await;

        let stream = listener
            .connect(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                // IpAddr::V4(Ipv4Addr::new(192, 168, 0, 144)),
                // 10001,
                7000,
            ))
            .await
            .unwrap();
        // let stream = listener.connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7000)).await.unwrap();
        stream.write(&buffer).await;
        stream.wait_for_termination().await;

        println!("Sent in {:?}", start.elapsed());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 10)]
    async fn recv_data() {
        let start = std::time::Instant::now();

        let listener = UtpListener::bind(std::net::SocketAddrV4::new(
            Ipv4Addr::new(0, 0, 0, 0),
            // 7000,
            10001,
        ))
        .await;

        println!("accepting..");
        let (stream, addr) = listener.accept().await;
        println!("accepted");

        let mut buffer = [0; 65000];

        loop {
            let res = stream.read(&mut buffer).await;

            if let Err(e) = res {
                println!("END ERROR={:?}", e);
                break;
            }
        }

        println!("DONEaaaa");

        // let stream = listener
        //     .connect(SocketAddr::new(
        //         IpAddr::V4(Ipv4Addr::new(192, 168, 0, 144)),
        //         7000,
        //     ))
        //     .await
        //     .unwrap();
        // // let stream = listener.connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7000)).await.unwrap();
        // stream.write(&buffer).await;
        // stream.wait_for_termination().await;

        // println!("Sent in {:?}", start.elapsed());
    }
}
