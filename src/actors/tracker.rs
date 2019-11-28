
use url::Url;
use async_std::sync::{Sender, Receiver, channel};
use async_std::{task, future};
use async_std::net::{SocketAddr, ToSocketAddrs};
use async_trait::async_trait;

use std::sync::Arc;
use std::time::{Duration, Instant};
use std::convert::{TryFrom, TryInto};
use std::io::Write;

use crate::metadata::Torrent;
use crate::supervisors::torrent::{Result, TorrentNotification};
use crate::http_client::{self, AnnounceQuery, AnnounceResponse};
use crate::session::get_peers_addrs;
use crate::actors::tracker_udp::{Action, ConnectRequest, ConnectResponse, AnnounceRequest, ScrapeRequest, ScrapeResponse};
use crate::errors::TorrentError;

#[async_trait]
trait TrackerConnection {
    async fn announce(&mut self, connected_addr: &mut usize) -> Result<Vec<SocketAddr>>;
    async fn scrape(&mut self) -> Result<()>;
}

pub struct UdpState {
    pub transaction_id: u32,
    pub connection_id: u64,
    connection_id_time: Instant,
    socket: UdpSocket
}

use smallvec::{SmallVec, smallvec};

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
    all_addrs_tried: bool,
}

struct HttpConnection {
    data: Arc<TrackerData>,
    addr: Vec<Arc<SocketAddr>>,
}

use async_std::net::UdpSocket;
use rand::prelude::*;
use crate::udp_ext::WithTimeout;
use async_std::io::ErrorKind;

impl UdpConnection {

    fn next_addr(&mut self) -> Arc<SocketAddr> {
        if (self.current_addr >= self.addrs.len()) {
            self.all_addrs_tried = true;
            self.current_addr = 0;
        }
        let addr = Arc::clone(&self.addrs[self.current_addr]);
        self.current_addr += 1;
        addr
    }

    fn current_addr(&self) -> &Arc<SocketAddr> {
        &self.addrs[self.current_addr]
    }

    async fn get_response<T>(&mut self, send_size: usize) -> Result<T>
    where
        T: TryFrom<super::tracker_udp::TrackerMessage, Error = TorrentError>
    {
        let mut attempts = 0;

        loop {
            println!("RETRY {:?} {:?}", attempts, self.addrs);
            if self.id_expired() {
                self.connect().await?;
            }

            let mut socket = &self.state.as_ref().unwrap().socket;

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
                Err(e) => Err(e)?
            };

            let buffer = self.buffer
                             .get(..n)
                             .ok_or(TorrentError::InvalidInput)?;

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
                Err(e) => Err(e)?
            };

            let buffer = self.buffer
                             .get(0..n)
                             .ok_or(TorrentError::InvalidInput)?;

            let resp: ConnectResponse = self.read_response(buffer)?.try_into()?;

            self.state = Some(UdpState {
                transaction_id,
                socket,
                connection_id: resp.connection_id,
                connection_id_time: Instant::now()
            });

            return Ok(resp)
        }
    }

    pub fn write_to_buffer(&mut self, msg: super::tracker_udp::TrackerMessage) -> usize {
        use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
        use std::io::Cursor;
        use super::tracker_udp::TrackerMessage;

        let mut buffer = &mut self.buffer;;

        // Cursor doesn't work with SmallVec
        match msg {
            TrackerMessage::ConnectReq(req) => {
                (&mut buffer[0..]).write_u64::<BigEndian>(req.protocol_id).unwrap();
                (&mut buffer[8..]).write_u32::<BigEndian>((&req.action).into()).unwrap();
                (&mut buffer[12..]).write_u32::<BigEndian>(req.transaction_id).unwrap();
                16
            }
            TrackerMessage::AnnounceReq(req) => {
                (&mut buffer[0..]).write_u64::<BigEndian>(req.connection_id).unwrap();
                (&mut buffer[8..]).write_u32::<BigEndian>((&req.action).into()).unwrap();
                (&mut buffer[12..]).write_u32::<BigEndian>(req.transaction_id).unwrap();
                (&mut buffer[16..]).write_all(&req.info_hash[..]).unwrap();
                (&mut buffer[36..]).write_all(req.peer_id.as_bytes()).unwrap();
                (&mut buffer[56..]).write_u64::<BigEndian>(req.downloaded).unwrap();
                (&mut buffer[64..]).write_u64::<BigEndian>(req.left).unwrap();
                (&mut buffer[72..]).write_u64::<BigEndian>(req.uploaded).unwrap();
                (&mut buffer[80..]).write_u32::<BigEndian>((&req.event).into()).unwrap();
                (&mut buffer[84..]).write_u32::<BigEndian>(req.ip_address).unwrap();
                (&mut buffer[88..]).write_u32::<BigEndian>(req.key).unwrap();
                (&mut buffer[92..]).write_u32::<BigEndian>(req.num_want).unwrap();
                (&mut buffer[96..]).write_u16::<BigEndian>(req.port).unwrap();
                98
            }
            TrackerMessage::ScrapeReq(req) => {
                (&mut buffer[0..]).write_u64::<BigEndian>(req.connection_id).unwrap();
                (&mut buffer[8..]).write_u32::<BigEndian>((&req.action).into()).unwrap();
                (&mut buffer[12..]).write_u32::<BigEndian>(req.transaction_id).unwrap();
                (&mut buffer[16..]).write_all(&req.info_hash[..]).unwrap();
                36
            }
            _ => unreachable!()
        }
    }

    pub fn read_response(&self, buffer: &[u8]) -> Result<super::tracker_udp::TrackerMessage> {
        use super::tracker_udp::{TrackerMessage, AnnounceResponse};
        use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
        use std::io::Cursor;

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
                    action, transaction_id, interval, leechers, seeders, addrs
                }))
            }
            Action::Scrape => {
                let complete = cursor.read_u32::<BigEndian>()?;
                let downloaded = cursor.read_u32::<BigEndian>()?;
                let incomplete = cursor.read_u32::<BigEndian>()?;

                Ok(TrackerMessage::ScrapeResp(ScrapeResponse {
                    action, transaction_id, complete, downloaded, incomplete
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
            _ => true
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

        let resp: super::tracker_udp::AnnounceResponse = self.get_response(n).await?;

        *addr = self.current_addr;

        Ok(resp.addrs)
    }

    async fn scrape(&mut self) -> Result<()> {
        if self.state.is_none() {
            self.connect().await?;
            self.buffer = smallvec![0; 16 * 1024];
        }

        let req = ScrapeRequest::from(&*self).into();
        let n = self.write_to_buffer(req);

        let resp: ScrapeResponse = self.get_response(n).await?;

        Ok(())
    }
}

#[async_trait]
impl TrackerConnection for HttpConnection {
    async fn announce(&mut self, connected_addr: &mut usize) -> Result<Vec<SocketAddr>> {
        let query = AnnounceQuery::from(self.data.metadata.as_ref());
        let mut last_err = None;
        for (index, addr) in self.addr.iter().enumerate() {
            let response = match http_client::get(&self.data.url, &query, addr).await {
                Ok(resp) => resp,
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            };
            *connected_addr = index;
            let peers = get_peers_addrs(&response);
            return Ok(peers);
        }
        match last_err {
            Some(e) => Err(e.into()),
            _ => Err(TorrentError::Unresponsive)
        }
    }

    async fn scrape(&mut self) -> Result<()> {
        Ok(())
    }
}

impl UdpConnection {
    fn new(
        data: Arc<TrackerData>,
        addrs: Vec<Arc<SocketAddr>>
    ) -> Box<dyn TrackerConnection + Send + Sync>
    {
        Box::new(Self { data, addrs, state: None, buffer: smallvec![0; 16], current_addr: 0, all_addrs_tried: false })
    }
}

impl HttpConnection {
    fn new(
        data: Arc<TrackerData>,
        addr: Vec<Arc<SocketAddr>>,
    ) -> Box<dyn TrackerConnection + Send + Sync>
    {
        Box::new(Self { data, addr })
    }
}

enum TrackerMessage {
    Found,
    HostUnresolved,
    ErrorOccured(TorrentError)
}

struct ATracker {
    data: Arc<TrackerData>,
    /// All addresses the host were resolved to
    /// When we're connected to an address, it is moved to the first position
    /// so later requests will use this address first.
    addrs: Vec<Arc<SocketAddr>>,
    tracker_supervisor: Sender<TrackerMessage>,
}

impl ATracker {
    fn new(data: Arc<TrackerData>, tracker_supervisor: Sender<TrackerMessage>) -> ATracker {
        ATracker { data, addrs: Vec::new(), tracker_supervisor }
    }

    fn set_connected_addr(&mut self, index: usize) {
        if index != 0 {
            self.addrs.swap(0, index);
        }
    }

    async fn connect_and_request(&mut self) -> Result<Vec<SocketAddr>> {
        let data = Arc::clone(&self.data);
        let mut connection = Self::new_connection(data, self.addrs.clone());

        let mut connected_index = 0;

        match connection.announce(&mut connected_index).await {
            Ok(addrs) if !addrs.is_empty() => {
                self.set_connected_addr(connected_index);
                Ok(addrs)
            },
            Ok(empty) => Ok(empty),
            Err(e) => {
                println!("ANNOUNCE FAILED {:?}", e);
                Err(e)
            }
        }
    }

    async fn resolve_and_start(&mut self) {
        use TrackerMessage::*;

        self.addrs = self.resolve_host().await;

        println!("RESOLVED ADDRS {:?}", self.addrs);

        if self.addrs.is_empty() {
            self.send_to_supervisor(HostUnresolved).await;
            return;
        }

        match self.connect_and_request().await {
            Ok(peer_addrs) => {
                println!("PEERS FOUND ! {:?}\nLENGTH = {:?}", peer_addrs, peer_addrs.len());
                self.send_to_supervisor(Found).await;
                self.send_addrs(peer_addrs).await;
            }
            Err(TorrentError::Unresponsive) => {
                self.send_to_supervisor(HostUnresolved).await;
            }
            Err(e) => {
                self.send_to_supervisor(ErrorOccured(e)).await;
            }
        }
    }

    async fn send_to_supervisor(&self, msg: TrackerMessage) {
        self.tracker_supervisor.send(msg).await;
    }

    async fn send_addrs(&self, addrs: Vec<SocketAddr>) {
        use TorrentNotification::PeerDiscovered;

        self.data.supervisor.send(PeerDiscovered { addrs }).await;
    }

    fn new_connection(
        data: Arc<TrackerData>,
        addr: Vec<Arc<SocketAddr>>,
    ) -> Box<dyn TrackerConnection + Send + Sync>
    {
        match data.url.scheme() {
            "http" => HttpConnection::new(data, addr),
            "udp" => UdpConnection::new(data, addr),
            _ => panic!("scheme was already checked")
        }
    }

    async fn resolve_host(&mut self) -> Vec<Arc<SocketAddr>> {
        let host = self.data.url.host_str().unwrap();
        let port = self.data.url.port().unwrap_or(80);

        (host, port).to_socket_addrs()
                    .await
                    .map(|a| a.map(Arc::new).collect())
                    .unwrap_or_else(|_| Vec::new())
    }
}

impl Drop for ATracker {
    fn drop(&mut self) {
        println!("ATRACKER DROPPED !", );
    }
}

pub struct TrackerData {
    pub metadata: Arc<Torrent>,
    pub supervisor: Sender<TorrentNotification>,
    pub url: Arc<Url>
}

impl From<(&Tracker, &Arc<Url>)> for TrackerData {
    fn from((tracker, url): (&Tracker, &Arc<Url>)) -> TrackerData {
        TrackerData {
            metadata: Arc::clone(&tracker.metadata),
            supervisor: tracker.supervisor.clone(),
            url: Arc::clone(url),
        }
    }
}

#[derive(Debug)]
pub struct Tracker {
    metadata: Arc<Torrent>,
    supervisor: Sender<TorrentNotification>,
    urls: Vec<Vec<Arc<Url>>>,
    recv: Receiver<TrackerMessage>,
    _sender: Sender<TrackerMessage>,
}

impl Tracker {
    pub fn new(
        supervisor: Sender<TorrentNotification>,
        metadata: Arc<Torrent>
    ) -> Tracker
    {
        let urls = metadata.get_urls_tiers();
        let (_sender, recv) = channel(10);
        Tracker { supervisor, metadata, urls, recv, _sender }
    }

    fn is_scheme_supported(url: &&Arc<Url>) -> bool {
        match url.scheme() {
            "http" | "udp" => true,
            _ => false
        }
    }

    pub async fn start(self) {
        use TrackerMessage::*;

        for tier in self.urls.as_slice() {

            for url in tier.iter().filter(Self::is_scheme_supported) {

                let data = Arc::new(TrackerData::from((&self, url)));
                let sender = self._sender.clone();

                task::spawn(async move {
                    ATracker::new(data, sender).resolve_and_start().await
                });

                let duration = Duration::from_secs(15);
                match future::timeout(duration, self.recv.recv()).await {
                    Ok(Some(Found)) => return,
                    _ => {}// TODO: Handle other cases
                }
            }
        }
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        println!("TRACKER DROPPED !", );
    }
}
