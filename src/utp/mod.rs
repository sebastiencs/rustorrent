use std::time::{SystemTime, UNIX_EPOCH};
use std::cmp::{PartialOrd, Ord};

pub mod socket;

/// A safe type using wrapping_{add,sub} for +/-/cmp operations
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
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
        (now.0 / 1000) - (self.0 / 1000)
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

use std::time::{Instant, Duration};

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

        if self.ndelays > 120 &&
            self.next_index_time
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
    IO(std::io::Error)
}

impl From<std::io::Error> for UtpError {
    fn from(e: std::io::Error) -> UtpError {
        UtpError::IO(e)
    }
}

type Result<T> = std::result::Result<T, UtpError>;

#[derive(Debug)]
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
            _ => Err(UtpError::WrongVersion)
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

#[repr(C, packed)]
struct Payload {
    data: [u8; 1500],
    len: usize
}

impl Payload {
    fn new(data: &[u8]) -> Payload {
        let data_len = data.len();
        let mut payload = [0; 1500];
        payload[..data_len].copy_from_slice(data);
        Payload {
            data: payload,
            len: data_len
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

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
//    resent_time: Timestamp,
}

impl std::fmt::Debug for Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Packet")
         .field("seq_nr", &self.seq_number)
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
    pub fn new(data: &[u8]) -> Packet {
        Packet {
            header: Header::default(),
            payload: Payload::new(data),
            seq_number: SequenceNumber::zero(),
            resent: false,
            last_sent: Timestamp::zero(),
        }
    }

    pub fn syn() -> Packet {
        Packet {
            header: Header::new(PacketType::Syn),
            payload: Payload::new(&[]),
            seq_number: SequenceNumber::zero(),
            resent: false,
            last_sent: Timestamp::zero(),
        }
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
        let slice = unsafe { &*(self as *const Packet as *const [u8; std::mem::size_of::<Packet>()]) };
        &slice[..std::mem::size_of::<Header>() + self.payload.len]
    }

    // pub fn iter_extensions(&self) -> ExtensionIterator {
    //     ExtensionIterator::new(self)
    // }
}

pub enum ExtensionType {
    SelectiveAck,
    None,
    Unknown
}

impl From<u8> for ExtensionType {
    fn from(byte: u8) -> ExtensionType {
        match byte {
            0 => ExtensionType::None,
            1 => ExtensionType::SelectiveAck,
            _ => ExtensionType::Unknown
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
    Missing(SequenceNumber)
}

impl Iterator for SelectiveAck<'_> {
    type Item = SelectiveAckBit;

    fn next(&mut self) -> Option<Self::Item> {
        if self.first {
            for byte in self.bitfield {
                println!("BITFIELD {:08b}", byte);
            }
            self.first = false;
            return Some(SelectiveAckBit::Missing(self.ack_number + 1));
        }

        let byte = *self.bitfield.get(self.byte_index)?;
        let bit = byte & (1 << self.bit_index);

        let ack_number = self.ack_number
            + self.byte_index as u16 * 8
            + self.bit_index as u16
            + 2;

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
    pub fn new(packet: &'a PacketRef) -> ExtensionIterator<'a> {
        let current_type = packet.get_extension_type();
        let slice = &packet.packet_ref.payload.data[..packet.len - HEADER_SIZE];
        let ack_number = packet.get_ack_number();

        for byte in &packet.packet_ref.payload.data[..packet.len - HEADER_SIZE] {
            println!("BYTE {:x}", byte);
        }
        ExtensionIterator { current_type, slice, ack_number }
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
                        first: true
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

pub struct PacketRef<'a> {
    packet_ref: &'a Packet,
    len: usize,
    received_at: Timestamp,
}

use std::ops::{Deref, DerefMut, Add, Sub, AddAssign, SubAssign};

impl Deref for PacketRef<'_> {
    type Target = Header;

    fn deref(&self) -> &Header {
        &self.packet_ref.header
    }
}

impl<'a> PacketRef<'a> {
    fn ref_from_buffer(buffer: &[u8]) -> Result<PacketRef> {
        let received_at = Timestamp::now();
        let len = buffer.len();
        if len < HEADER_SIZE {
            return Err(UtpError::Malformed);
        }
        Ok(PacketRef {
            len,
            received_at,
            packet_ref: unsafe { &*(buffer.as_ptr() as *const Packet) },
        })
    }

    fn header(&self) -> &Header {
        &self.packet_ref.header
    }

    pub fn iter_extensions(&self) -> ExtensionIterator {
        ExtensionIterator::new(self)
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

pub const UDP_IPV4_OVERHEAD: usize = (IPV4_HEADER_SIZE + UDP_HEADER_SIZE);
pub const UDP_IPV6_OVERHEAD: usize = (IPV6_HEADER_SIZE + UDP_HEADER_SIZE);
pub const UDP_TEREDO_OVERHEAD: usize = (UDP_IPV4_OVERHEAD + UDP_IPV6_OVERHEAD);

pub const UDP_IPV4_MTU: usize =
    (ETHERNET_MTU - IPV4_HEADER_SIZE - UDP_HEADER_SIZE - GRE_HEADER_SIZE
     - PPPOE_HEADER_SIZE - MPPE_HEADER_SIZE - FUDGE_HEADER_SIZE);

pub const UDP_IPV6_MTU: usize =
    (ETHERNET_MTU - IPV6_HEADER_SIZE - UDP_HEADER_SIZE - GRE_HEADER_SIZE
     - PPPOE_HEADER_SIZE - MPPE_HEADER_SIZE - FUDGE_HEADER_SIZE);

pub const UDP_TEREDO_MTU: usize = (TEREDO_MTU - IPV6_HEADER_SIZE - UDP_HEADER_SIZE);
