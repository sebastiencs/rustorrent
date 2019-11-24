use std::io::Cursor;
use std::convert::TryFrom;
use std::io::Write;
use std::convert::TryInto;

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

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

#[derive(Debug)]
pub struct ConnectResponse {
    pub action: Action,
    pub transaction_id: u32,
    pub connection_id: u64,
}

#[derive(Debug)]
pub struct AnnounceRequest {
    pub connection_id: u64,
    pub action: Action,
    pub transaction_id: u32,
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
    pub downloaded: u64,
    pub left: u64,
    pub uploaded: u64,
    pub event: u32,
    pub ip_address: u32,
    pub key: u32,
    pub num_want: u32,
    pub port: u16,
}

#[derive(Debug)]
pub struct AnnounceResponse<'a> {
    pub action: Action,
    pub transaction_id: u32,
    pub interval: u32,
    pub leechers: u32,
    pub seeders: u32,
    pub ip_port_slice: &'a [u8],
}

#[derive(Debug)]
pub enum TrackerMessage<'a> {
    ConnectReq(ConnectRequest),
    ConnectResp(ConnectResponse),
    AnnounceReq(AnnounceRequest),
    AnnounceResp(AnnounceResponse<'a>),
}

impl From<ConnectRequest> for TrackerMessage<'static> {
    fn from(r: ConnectRequest) -> TrackerMessage<'static> {
        TrackerMessage::ConnectReq(r)
    }
}
impl From<ConnectResponse> for TrackerMessage<'static> {
    fn from(r: ConnectResponse) -> TrackerMessage<'static> {
        TrackerMessage::ConnectResp(r)
    }
}

impl From<AnnounceRequest> for TrackerMessage<'static> {
    fn from(r: AnnounceRequest) -> TrackerMessage<'static> {
        TrackerMessage::AnnounceReq(r)
    }
}
impl<'a> From<AnnounceResponse<'a>> for TrackerMessage<'a> {
    fn from(r: AnnounceResponse<'a>) -> TrackerMessage<'a> {
        TrackerMessage::AnnounceResp(r)
    }
}

pub fn write_message(msg: TrackerMessage, buffer: &mut Vec<u8>) {
    match msg {
        TrackerMessage::ConnectReq(req) => {
            let mut cursor = Cursor::new(buffer);
            cursor.write_u64::<BigEndian>(req.protocol_id).unwrap();
            cursor.write_u32::<BigEndian>((&req.action).into()).unwrap();
            cursor.write_u32::<BigEndian>(req.transaction_id).unwrap();
        }
        TrackerMessage::AnnounceReq(req) => {
            let mut cursor = Cursor::new(buffer);
            cursor.write_u64::<BigEndian>(req.connection_id).unwrap();
            cursor.write_u32::<BigEndian>((&req.action).into()).unwrap();
            cursor.write_u32::<BigEndian>(req.transaction_id).unwrap();
            cursor.write_all(&req.info_hash[..]).unwrap();
            cursor.write_all(&req.peer_id[..]).unwrap();
            cursor.write_u64::<BigEndian>(req.downloaded).unwrap();
            cursor.write_u64::<BigEndian>(req.left).unwrap();
            cursor.write_u64::<BigEndian>(req.uploaded).unwrap();
            cursor.write_u32::<BigEndian>(req.event).unwrap();
            cursor.write_u32::<BigEndian>(req.ip_address).unwrap();
            cursor.write_u32::<BigEndian>(req.key).unwrap();
            cursor.write_u32::<BigEndian>(req.num_want).unwrap();
            cursor.write_u16::<BigEndian>(req.port).unwrap();
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
            let interval = cursor.read_u32::<BigEndian>()?;
            let leechers = cursor.read_u32::<BigEndian>()?;
            let seeders = cursor.read_u32::<BigEndian>()?;
            let ip_port_slice = &buffer[cursor.position() as usize..];
            Ok(TrackerMessage::AnnounceResp(AnnounceResponse {
                action, transaction_id, interval, leechers, seeders, ip_port_slice
            }))
        }
        Action::Scrape => {
            unreachable!()
        }
    }
}
