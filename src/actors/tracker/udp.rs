use std::{
    convert::{TryFrom, TryInto},
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use std::net::SocketAddr;

use super::{TrackerConnection, TrackerData};
use crate::{errors::TorrentError, peer::peer::PeerExternId, supervisors::torrent::Result};

#[derive(Debug)]
pub enum Action {
    Connect,
    Announce,
    Scrape,
    Error,
}

impl TryFrom<u32> for Action {
    type Error = TorrentError;

    fn try_from(n: u32) -> Result<Action> {
        match n {
            0 => Ok(Action::Connect),
            1 => Ok(Action::Announce),
            2 => Ok(Action::Scrape),
            3 => Ok(Action::Error),
            _ => Err(TorrentError::InvalidInput),
        }
    }
}

impl From<&Action> for u32 {
    fn from(action: &Action) -> Self {
        match action {
            Action::Connect => 0,
            Action::Announce => 1,
            Action::Scrape => 2,
            Action::Error => 3,
        }
    }
}

#[derive(Debug)]
pub struct ConnectRequest {
    pub protocol_id: u64,
    pub action: Action,
    pub transaction_id: u32,
}

impl ConnectRequest {
    pub fn new(transaction_id: u32) -> ConnectRequest {
        ConnectRequest {
            transaction_id,
            protocol_id: 0x0417_2710_1980,
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
    Stopped,
}

impl TryFrom<u32> for Event {
    type Error = TorrentError;

    fn try_from(n: u32) -> Result<Event> {
        match n {
            0 => Ok(Event::None),
            1 => Ok(Event::Completed),
            2 => Ok(Event::Started),
            3 => Ok(Event::Stopped),
            _ => Err(TorrentError::InvalidInput),
        }
    }
}

impl From<&Event> for u32 {
    fn from(e: &Event) -> Self {
        match e {
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
    pub info_hash: Arc<[u8]>,
    //    pub info_hash: [u8; 20],
    pub peer_id: Arc<PeerExternId>,
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

#[derive(Debug)]
pub struct ScrapeRequest {
    pub connection_id: u64,
    pub action: Action,
    pub transaction_id: u32,
    pub info_hash: Arc<[u8]>, // TODO: Handle more hash
}

impl<'a> From<&'a UdpConnection> for AnnounceRequest {
    fn from(c: &'a UdpConnection) -> AnnounceRequest {
        let metadata = &c.data.metadata;
        let state = c.state.as_ref().unwrap();
        AnnounceRequest {
            connection_id: state.connection_id,
            action: Action::Announce,
            transaction_id: state.transaction_id,
            info_hash: Arc::clone(&metadata.info_hash),
            peer_id: Arc::clone(&c.data.extern_id),
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

impl<'a> From<&'a UdpConnection> for ScrapeRequest {
    fn from(c: &'a UdpConnection) -> ScrapeRequest {
        let metadata = &c.data.metadata;
        let state = c.state.as_ref().unwrap();
        ScrapeRequest {
            connection_id: state.connection_id,
            action: Action::Scrape,
            transaction_id: state.transaction_id,
            info_hash: Arc::clone(&metadata.info_hash),
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
}

#[derive(Debug)]
pub struct ScrapeResponse {
    pub action: Action,
    pub transaction_id: u32,
    // TODO: Handle more torrent
    pub complete: u32,
    pub downloaded: u32,
    pub incomplete: u32,
}

#[derive(Debug)]
pub enum TrackerMessage {
    ConnectReq(ConnectRequest),
    ConnectResp(ConnectResponse),
    AnnounceReq(AnnounceRequest),
    AnnounceResp(AnnounceResponse),
    ScrapeReq(ScrapeRequest),
    ScrapeResp(ScrapeResponse),
}

impl TryFrom<TrackerMessage> for ConnectResponse {
    type Error = TorrentError;
    fn try_from(msg: TrackerMessage) -> Result<ConnectResponse> {
        match msg {
            TrackerMessage::ConnectResp(res) => Ok(res),
            _ => Err(TorrentError::InvalidInput),
        }
    }
}

impl TryFrom<TrackerMessage> for ScrapeResponse {
    type Error = TorrentError;
    fn try_from(msg: TrackerMessage) -> Result<ScrapeResponse> {
        match msg {
            TrackerMessage::ScrapeResp(res) => Ok(res),
            _ => Err(TorrentError::InvalidInput),
        }
    }
}

impl TryFrom<TrackerMessage> for AnnounceResponse {
    type Error = TorrentError;
    fn try_from(msg: TrackerMessage) -> Result<AnnounceResponse> {
        match msg {
            TrackerMessage::AnnounceResp(res) => Ok(res),
            _ => Err(TorrentError::InvalidInput),
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
impl From<ScrapeRequest> for TrackerMessage {
    fn from(r: ScrapeRequest) -> TrackerMessage {
        TrackerMessage::ScrapeReq(r)
    }
}
impl From<ScrapeResponse> for TrackerMessage {
    fn from(r: ScrapeResponse) -> TrackerMessage {
        TrackerMessage::ScrapeResp(r)
    }
}

pub struct UdpState {
    pub transaction_id: u32,
    pub connection_id: u64,
    connection_id_time: Instant,
    socket: UdpSocket,
}

use smallvec::{smallvec, SmallVec};

pub struct UdpConnection {
    pub data: Arc<TrackerData>,
    pub addrs: Vec<Arc<SocketAddr>>,
    pub state: Option<UdpState>,
    /// 16 bytes is just enough to send/receive Connect
    /// We allocate more after this request
    /// This avoid to allocate when the tracker is unreachable
    buffer: SmallVec<[u8; 16]>,
    current_addr: usize,
    /// true if we tried to connect to all [`addrs`]
    /// This avoid to give up when there are remaining addresses
    /// even after some delay
    all_addrs_tried: bool,
}

use tokio::net::UdpSocket;

use crate::udp_ext::WithTimeout;
use tokio::io::ErrorKind;

impl UdpConnection {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        data: Arc<TrackerData>,
        addrs: Vec<Arc<SocketAddr>>,
    ) -> Box<dyn TrackerConnection + Send + Sync> {
        Box::new(Self {
            data,
            addrs,
            state: None,
            buffer: smallvec![0; 16],
            current_addr: 0,
            all_addrs_tried: false,
        })
    }

    fn next_addr(&mut self) -> &Arc<SocketAddr> {
        if self.current_addr >= self.addrs.len() {
            self.all_addrs_tried = true;
            self.current_addr = 0;
        }
        let addr = &self.addrs[self.current_addr];
        self.current_addr += 1;
        addr
    }

    fn current_addr(&self) -> &Arc<SocketAddr> {
        &self.addrs[self.current_addr - 1]
    }

    async fn get_response<T>(&mut self, send_size: usize) -> Result<T>
    where
        T: TryFrom<TrackerMessage, Error = TorrentError>,
    {
        let mut attempts = 0;

        loop {
            println!("RETRY {:?} {:?}", attempts, self.addrs);
            if self.id_expired() {
                self.connect().await?;
            }

            let socket = &self.state.as_ref().unwrap().socket;

            socket.send(&self.buffer[..send_size]).await?;

            let timeout = Duration::from_secs(10);

            let n = match socket.recv_timeout(&mut self.buffer, timeout).await {
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::TimedOut => {
                    if attempts == 6 {
                        return Err(TorrentError::Unresponsive);
                    }
                    attempts += 1;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let buffer = self.buffer.get(..n).ok_or(TorrentError::InvalidInput)?;

            return T::try_from(self.read_response(buffer)?);
        }
    }

    async fn connect(&mut self) -> Result<ConnectResponse> {
        let mut attempts = 0;

        loop {
            let socket = UdpSocket::bind("0:0").await?;

            socket.connect(self.next_addr().as_ref()).await?;

            println!("RETRY CONNECT {:?} {:?}", self.addrs, socket.local_addr());

            let transaction_id = rand::random();

            self.write_to_buffer(ConnectRequest::new(transaction_id).into());

            socket.send(&self.buffer[..16]).await?;

            let timeout = Duration::from_secs(10);

            let n = match socket.recv_timeout(&mut self.buffer, timeout).await {
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::TimedOut => {
                    if attempts >= 6 && self.all_addrs_tried {
                        return Err(TorrentError::Unresponsive);
                    }
                    attempts += 1;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let buffer = self.buffer.get(0..n).ok_or(TorrentError::InvalidInput)?;

            let resp: ConnectResponse = self.read_response(buffer)?.try_into()?;

            self.state = Some(UdpState {
                transaction_id,
                socket,
                connection_id: resp.connection_id,
                connection_id_time: Instant::now(),
            });

            return Ok(resp);
        }
    }

    pub fn write_to_buffer(&mut self, msg: TrackerMessage) -> usize {
        use byteorder::{BigEndian, WriteBytesExt};

        let buffer = &mut self.buffer;

        // Cursor doesn't work with SmallVec
        match msg {
            TrackerMessage::ConnectReq(req) => {
                (&mut buffer[0..])
                    .write_u64::<BigEndian>(req.protocol_id)
                    .unwrap();
                (&mut buffer[8..])
                    .write_u32::<BigEndian>((&req.action).into())
                    .unwrap();
                (&mut buffer[12..])
                    .write_u32::<BigEndian>(req.transaction_id)
                    .unwrap();
                16
            }
            TrackerMessage::AnnounceReq(req) => {
                (&mut buffer[0..])
                    .write_u64::<BigEndian>(req.connection_id)
                    .unwrap();
                (&mut buffer[8..])
                    .write_u32::<BigEndian>((&req.action).into())
                    .unwrap();
                (&mut buffer[12..])
                    .write_u32::<BigEndian>(req.transaction_id)
                    .unwrap();
                (&mut buffer[16..]).write_all(&req.info_hash[..]).unwrap();
                (&mut buffer[36..]).write_all(&**req.peer_id).unwrap();
                (&mut buffer[56..])
                    .write_u64::<BigEndian>(req.downloaded)
                    .unwrap();
                (&mut buffer[64..])
                    .write_u64::<BigEndian>(req.left)
                    .unwrap();
                (&mut buffer[72..])
                    .write_u64::<BigEndian>(req.uploaded)
                    .unwrap();
                (&mut buffer[80..])
                    .write_u32::<BigEndian>((&req.event).into())
                    .unwrap();
                (&mut buffer[84..])
                    .write_u32::<BigEndian>(req.ip_address)
                    .unwrap();
                (&mut buffer[88..]).write_u32::<BigEndian>(req.key).unwrap();
                (&mut buffer[92..])
                    .write_u32::<BigEndian>(req.num_want)
                    .unwrap();
                (&mut buffer[96..])
                    .write_u16::<BigEndian>(req.port)
                    .unwrap();
                98
            }
            TrackerMessage::ScrapeReq(req) => {
                (&mut buffer[0..])
                    .write_u64::<BigEndian>(req.connection_id)
                    .unwrap();
                (&mut buffer[8..])
                    .write_u32::<BigEndian>((&req.action).into())
                    .unwrap();
                (&mut buffer[12..])
                    .write_u32::<BigEndian>(req.transaction_id)
                    .unwrap();
                (&mut buffer[16..]).write_all(&req.info_hash[..]).unwrap();
                36
            }
            _ => unreachable!(),
        }
    }

    pub fn read_response(&self, buffer: &[u8]) -> Result<TrackerMessage> {
        use byteorder::{BigEndian, ReadBytesExt};
        use std::io::Cursor;

        let mut cursor = Cursor::new(buffer);
        let action = cursor.read_u32::<BigEndian>()?.try_into()?;
        let transaction_id = cursor.read_u32::<BigEndian>()?;
        match action {
            Action::Connect => {
                let connection_id = cursor.read_u64::<BigEndian>()?;
                Ok(TrackerMessage::ConnectResp(ConnectResponse {
                    action,
                    transaction_id,
                    connection_id,
                }))
            }
            Action::Announce => {
                let interval = cursor.read_u32::<BigEndian>()?;
                let leechers = cursor.read_u32::<BigEndian>()?;
                let seeders = cursor.read_u32::<BigEndian>()?;
                let slice_addrs = &buffer[cursor.position() as usize..];

                // TODO: When available, use the function UdpSocket::peer_addr
                let addrs = if self.current_addr().is_ipv4() {
                    let mut addrs = Vec::with_capacity(slice_addrs.len() / 6);
                    crate::utils::ipv4_from_slice(slice_addrs, &mut addrs);
                    addrs
                } else {
                    let mut addrs = Vec::with_capacity(slice_addrs.len() / 18);
                    crate::utils::ipv6_from_slice(slice_addrs, &mut addrs);
                    addrs
                };
                Ok(TrackerMessage::AnnounceResp(AnnounceResponse {
                    action,
                    transaction_id,
                    interval,
                    leechers,
                    seeders,
                    addrs,
                }))
            }
            Action::Scrape => {
                let complete = cursor.read_u32::<BigEndian>()?;
                let downloaded = cursor.read_u32::<BigEndian>()?;
                let incomplete = cursor.read_u32::<BigEndian>()?;

                Ok(TrackerMessage::ScrapeResp(ScrapeResponse {
                    action,
                    transaction_id,
                    complete,
                    downloaded,
                    incomplete,
                }))
            }
            Action::Error => {
                unreachable!() // TODO
            }
        }
    }

    pub fn id_expired(&self) -> bool {
        match self.state.as_ref() {
            Some(state) => Instant::now() - state.connection_id_time >= Duration::from_secs(60),
            _ => true,
        }
    }
}

#[async_trait]
impl TrackerConnection for UdpConnection {
    async fn announce(&mut self, addr: &mut usize) -> Result<Vec<SocketAddr>> {
        if self.state.is_none() {
            self.connect().await?;
            self.buffer = smallvec![0; 16 * 1024];
        }

        let req = AnnounceRequest::from(&*self).into();
        let n = self.write_to_buffer(req);

        let resp: AnnounceResponse = self.get_response(n).await?;

        *addr = self.current_addr - 1;

        Ok(resp.addrs)
    }

    async fn scrape(&mut self) -> Result<()> {
        if self.state.is_none() {
            self.connect().await?;
            self.buffer = smallvec![0; 16 * 1024];
        }

        let req = ScrapeRequest::from(&*self).into();
        let n = self.write_to_buffer(req);

        let _resp: ScrapeResponse = self.get_response(n).await?;

        Ok(())
    }
}
