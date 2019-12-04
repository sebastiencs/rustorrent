use std::time::{SystemTime, UNIX_EPOCH};

pub mod socket;

#[derive(Debug, Copy, Clone)]
pub struct Timestamp(u32);

impl Timestamp {
    pub fn now() -> Timestamp {
        let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        since_epoch.subsec_micros().into()
    }

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
pub struct Delay(i64);

impl Delay {
    pub fn since(timestamp: Timestamp) -> Delay {
        let now = Timestamp::now();
        now.delay(timestamp)
    }

    pub fn infinity() -> Delay {
        Delay(i64::max_value())
    }

    pub fn as_num(&self) -> i64 {
        self.0
    }
}

impl Sub for Delay {
    type Output = Delay;

    fn sub(self, other: Delay) -> Delay {
        Delay(self.0 - other.0)
    }
}

impl From<u32> for Delay {
    fn from(n: u32) -> Delay {
        Delay(n as i64)
    }
}

impl From<i64> for Delay {
    fn from(n: i64) -> Delay {
        Delay(n)
    }
}

impl Into<u32> for Delay {
    fn into(self) -> u32 {
        self.0 as u32
    }
}

#[derive(Debug)]
pub enum UtpError {
    Malformed,
    UnknownPacketType,
    WrongVersion,
    FamillyMismatch,
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
    fn get_seq_number(&self) -> u16 {
        u16::from_be(self.seq_nr)
    }
    fn get_ack_number(&self) -> u16 {
        u16::from_be(self.ack_nr)
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
    fn set_seq_number(&mut self, seq_number: u16) {
        self.seq_nr = u16::to_be(seq_number);
    }
    fn set_ack_number(&mut self, ack_number: u16) {
        self.ack_nr = u16::to_be(ack_number);
    }

    fn update_timestamp(&mut self) {
        self.set_timestamp(Timestamp::now());
    }

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
    payload: [u8; 1500],
    len: u16
}

#[repr(C, packed)]
pub struct Packet {
    header: Header,
    payload: Payload
}

impl Deref for Packet {
    type Target = Header;

    fn deref(&self) -> &Header {
        &self.header
    }
}

pub struct PacketRef<'a> {
    packet_ref: &'a Packet,
    len: usize
}

use std::ops::{Deref, Add, Sub};

impl Deref for PacketRef<'_> {
    type Target = Header;

    fn deref(&self) -> &Header {
        &self.packet_ref.header
    }
}

impl<'a> PacketRef<'a> {
    fn ref_from_buffer(buffer: &[u8]) -> Result<PacketRef> {
        if buffer.len() < std::mem::size_of::<Header>() {
            return Err(UtpError::Malformed);
        }
        Ok(PacketRef {
            packet_ref: unsafe { &*(buffer.as_ptr() as *const Packet) },
            len: buffer.len()
        })
    }

    fn header(&self) -> &Header {
        &self.packet_ref.header
    }
}
