use std::io::Cursor;
use std::convert::TryFrom;
use std::io::Write;
use std::convert::TryInto;
use std::sync::Arc;

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use async_std::net::SocketAddr;

use crate::supervisors::torrent::Result;
use crate::errors::TorrentError;

#[derive(Debug)]
pub enum Action {
    Connect,
    Announce,
    Scrape
}

impl TryFrom<u32> for Action {
    type Error = TorrentError;

    fn try_from(n :u32) -> Result<Action> {
        match n {
            0 => Ok(Action::Connect),
            1 => Ok(Action::Announce),
            2 => Ok(Action::Scrape),
            _ => Err(TorrentError::InvalidInput)
        }
    }
}

impl Into<u32> for &Action {
    fn into(self) -> u32 {
        match self {
            Action::Connect => 0,
            Action::Announce => 1,
            Action::Scrape => 2,
        }
    }
}

#[derive(Debug)]
pub struct ConnectRequest {
    pub protocol_id: u64,
    pub action: Action,
    pub transaction_id: u32
}

impl ConnectRequest {
    pub fn new(transaction_id: u32) -> ConnectRequest {
        ConnectRequest {
            transaction_id,
            protocol_id: 0x41727101980,
            action: Action::Connect,
        }
    }
}

#[derive(Debug)]
pub struct ConnectResponse {
    pub action: Action,
    pub transaction_id: u32,
    pub connection_id: u64,
}

#[derive(Debug)]
pub enum Event {
    None,
    Completed,
    Started,
    Stopped
}

impl TryFrom<u32> for Event {
    type Error = TorrentError;

    fn try_from(n :u32) -> Result<Event> {
        match n {
            0 => Ok(Event::None),
            1 => Ok(Event::Completed),
            2 => Ok(Event::Started),
            3 => Ok(Event::Stopped),
            _ => Err(TorrentError::InvalidInput)
        }
    }
}

impl Into<u32> for &Event {
    fn into(self) -> u32 {
        match self {
            Event::None => 0,
            Event::Completed => 1,
            Event::Started => 2,
            Event::Stopped => 3,
        }
    }
}

#[derive(Debug)]
pub struct AnnounceRequest {
    pub connection_id: u64,
    pub action: Action,
    pub transaction_id: u32,
    pub info_hash: Arc<Vec<u8>>,
//    pub info_hash: [u8; 20],
    pub peer_id: String,
//    pub peer_id: [u8; 20],
    pub downloaded: u64,
    pub left: u64,
    pub uploaded: u64,
    pub event: Event,
    pub ip_address: u32,
    pub key: u32,
    pub num_want: u32,
    pub port: u16,
}

use crate::actors::tracker::UdpConnection;

impl<'a> From<&'a UdpConnection> for AnnounceRequest {
    fn from(c: &'a UdpConnection) -> AnnounceRequest {
        let metadata = &c.data.metadata;
        let state = c.state.as_ref().unwrap();
        AnnounceRequest {
            connection_id: state.connection_id,
            action: Action::Announce,
            transaction_id: state.transaction_id,
            info_hash: Arc::clone(&metadata.info_hash),
            peer_id: "-RT1220sJ1Nna5rzWLd8".to_owned(),
            downloaded: 0,
            left: metadata.files_total_size() as u64,
            uploaded: 0,
            event: Event::Started,
            ip_address: 0,
            key: 0,
            num_want: 100,
            port: 6881,
        }
    }
}

#[derive(Debug)]
pub struct AnnounceResponse {
    pub action: Action,
    pub transaction_id: u32,
    pub interval: u32,
    pub leechers: u32,
    pub seeders: u32,
    pub addrs: Vec<SocketAddr>,
    //pub ip_port_slice: &'a [u8],
}

#[derive(Debug)]
pub enum TrackerMessage {
    ConnectReq(ConnectRequest),
    ConnectResp(ConnectResponse),
    AnnounceReq(AnnounceRequest),
    AnnounceResp(AnnounceResponse),
}

impl TryFrom<TrackerMessage> for ConnectResponse {
    type Error = TorrentError;
    fn try_from(msg: TrackerMessage) -> Result<ConnectResponse> {
        match msg {
            TrackerMessage::ConnectResp(res) => Ok(res),
            _ => Err(TorrentError::InvalidInput)
        }
    }
}

impl TryFrom<TrackerMessage> for AnnounceResponse {
    type Error = TorrentError;
    fn try_from(msg: TrackerMessage) -> Result<AnnounceResponse> {
        match msg {
            TrackerMessage::AnnounceResp(res) => Ok(res),
            _ => Err(TorrentError::InvalidInput)
        }
    }
}

impl From<ConnectRequest> for TrackerMessage {
    fn from(r: ConnectRequest) -> TrackerMessage {
        TrackerMessage::ConnectReq(r)
    }
}
impl From<ConnectResponse> for TrackerMessage {
    fn from(r: ConnectResponse) -> TrackerMessage {
        TrackerMessage::ConnectResp(r)
    }
}

impl From<AnnounceRequest> for TrackerMessage {
    fn from(r: AnnounceRequest) -> TrackerMessage {
        TrackerMessage::AnnounceReq(r)
    }
}
impl From<AnnounceResponse> for TrackerMessage {
    fn from(r: AnnounceResponse) -> TrackerMessage {
        TrackerMessage::AnnounceResp(r)
    }
}

//pub fn write_message(msg: TrackerMessage, buffer: &mut Vec<u8>) {
pub fn write_message(msg: TrackerMessage, buffer: &mut [u8]) -> usize {
    match msg {
        TrackerMessage::ConnectReq(req) => {
            let mut cursor = Cursor::new(buffer);
            cursor.write_u64::<BigEndian>(req.protocol_id).unwrap();
            cursor.write_u32::<BigEndian>((&req.action).into()).unwrap();
            cursor.write_u32::<BigEndian>(req.transaction_id).unwrap();
            cursor.position() as usize
        }
        TrackerMessage::AnnounceReq(req) => {
            let mut cursor = Cursor::new(buffer);
            cursor.write_u64::<BigEndian>(req.connection_id).unwrap();
            cursor.write_u32::<BigEndian>((&req.action).into()).unwrap();
            cursor.write_u32::<BigEndian>(req.transaction_id).unwrap();
            cursor.write_all(&req.info_hash[..]).unwrap();
            cursor.write_all(req.peer_id.as_bytes()).unwrap();
            cursor.write_u64::<BigEndian>(req.downloaded).unwrap();
            cursor.write_u64::<BigEndian>(req.left).unwrap();
            cursor.write_u64::<BigEndian>(req.uploaded).unwrap();
            cursor.write_u32::<BigEndian>((&req.event).into()).unwrap();
            cursor.write_u32::<BigEndian>(req.ip_address).unwrap();
            cursor.write_u32::<BigEndian>(req.key).unwrap();
            cursor.write_u32::<BigEndian>(req.num_want).unwrap();
            cursor.write_u16::<BigEndian>(req.port).unwrap();
            cursor.position() as usize
        }
        _ => unreachable!()
    }
}

pub fn read_response(buffer: &[u8]) -> Result<TrackerMessage> {
    let mut cursor = Cursor::new(buffer);
    let action = cursor.read_u32::<BigEndian>()?.try_into()?;
    let transaction_id = cursor.read_u32::<BigEndian>()?;
    match action {
        Action::Connect => {
            let connection_id = cursor.read_u64::<BigEndian>()?;
            Ok(TrackerMessage::ConnectResp(ConnectResponse {
                action, transaction_id, connection_id
            }))
        }
        Action::Announce => {
            unreachable!()
            // let interval = cursor.read_u32::<BigEndian>()?;
            // let leechers = cursor.read_u32::<BigEndian>()?;
            // let seeders = cursor.read_u32::<BigEndian>()?;
            // let ip_port_slice = &buffer[cursor.position() as usize..];
            // Ok(TrackerMessage::AnnounceResp(AnnounceResponse {
            //     action, transaction_id, interval, leechers, seeders, ip_port_slice
            // }))
        }
        Action::Scrape => {
            unreachable!()
        }
    }
}
