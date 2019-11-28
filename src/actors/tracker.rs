
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
    async fn announce(&mut self) -> Result<Vec<SocketAddr>>;
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
    pub addr: Arc<SocketAddr>,
    pub state: Option<UdpState>,
    /// 16 bytes is just enough to send/receive Connect
    /// We allocate more after this request
    buffer: SmallVec<[u8; 16]>,
}

struct HttpConnection {
    data: Arc<TrackerData>,
    addr: Arc<SocketAddr>,
}

use async_std::net::UdpSocket;
use rand::prelude::*;
use crate::udp_ext::WithTimeout;
use async_std::io::ErrorKind;

impl UdpConnection {

    async fn get_response<T>(&mut self, send_size: usize) -> Result<T>
    where
        T: TryFrom<super::tracker_udp::TrackerMessage, Error = TorrentError>
    {
        let mut tried = 0;

        loop {
            if self.id_expired() {
                self.connect().await?;
            }

            let mut socket = &self.state.as_ref().unwrap().socket;

            socket.send(&self.buffer[..send_size]).await?;

            let timeout = Duration::from_secs(15 * 2u64.pow(tried));

            let n = match socket.recv_timeout(&mut self.buffer, timeout).await {
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::TimedOut => {
                    tried += 1;
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
        let socket = UdpSocket::bind("0.0.0.0:0").await?;

        socket.connect(self.addr.as_ref()).await?;

        let transaction_id = rand::random();

        self.write_to_buffer(ConnectRequest::new(transaction_id).into());

        let mut tried = 0;

        // We don't use Self::get_response here because it would make a
        // recursion
        // https://rust-lang.github.io/async-book/07_workarounds/05_recursion.html
        loop {
            socket.send(&self.buffer[..16]).await?;

            let timeout = Duration::from_secs(15 * 2u64.pow(tried));

            let n = match socket.recv_timeout(&mut self.buffer, timeout).await {
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::TimedOut => {
                    tried += 1;
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
                let addrs = if self.addr.is_ipv4() {
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
    async fn announce(&mut self) -> Result<Vec<SocketAddr>> {
        if self.state.is_none() {
            self.connect().await?;
            self.buffer = smallvec![0; 16 * 1024];
        }

        let req = AnnounceRequest::from(&*self).into();
        let n = self.write_to_buffer(req);

        let resp: super::tracker_udp::AnnounceResponse = self.get_response(n).await?;

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
    async fn announce(&mut self) -> Result<Vec<SocketAddr>> {
        let query = AnnounceQuery::from(self.data.metadata.as_ref());
        let response = http_client::get(&self.data.url, query, self.addr.as_ref()).await?;

        let peers = get_peers_addrs(&response);

        Ok(peers)
    }

    async fn scrape(&mut self) -> Result<()> {
        Ok(())
    }
}

impl UdpConnection {
    fn new(
        data: Arc<TrackerData>,
        addr: Arc<SocketAddr>
    ) -> Box<dyn TrackerConnection + Send + Sync>
    {
        Box::new(Self { data, addr, state: None, buffer: smallvec![0; 16] })
    }
}

impl HttpConnection {
    fn new(
        data: Arc<TrackerData>,
        addr: Arc<SocketAddr>,
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
    addrs: Vec<Arc<SocketAddr>>,
    /// Last address we were connected to
    /// It's one of `addrs`
    last_addr: Option<Arc<SocketAddr>>,
    tracker_supervisor: Sender<TrackerMessage>,
}

impl ATracker {
    fn new(data: Arc<TrackerData>, tracker_supervisor: Sender<TrackerMessage>) -> ATracker {
        ATracker { data, addrs: Vec::new(), last_addr: None, tracker_supervisor }
    }

    async fn resolve_and_start(&mut self) {
        use TrackerMessage::*;

        self.addrs = self.resolve_host().await;
        let addrs = self.addrs.as_slice();

        if addrs.is_empty() {
            self.send_to_supervisor(HostUnresolved).await;
            return;
        }

        let mut last_error = None;

        for addr in addrs {
            let data = Arc::clone(&self.data);
            let mut connection = Self::new_connection(data, Arc::clone(addr));

            println!("CONNECTING TO {:?} {:?}", self.data.url, addr);
            let peer_addrs = match connection.announce().await {
                Ok(addrs) if !addrs.is_empty() => addrs,
                Ok(_) => continue,
                Err(e) => {
                    println!("ANNOUNCE FAILED {:?}", e);
                    last_error = Some(e);
                    continue;
                }
            };

            println!("PEERS FOUND ! {:?}\nLENGTH = {:?}", peer_addrs, peer_addrs.len());

            self.send_to_supervisor(Found).await;
            self.send_addrs(peer_addrs).await;
            self.last_addr = Some(Arc::clone(addr));

            return;
        }

        if let Some(e) = last_error {
            self.send_to_supervisor(ErrorOccured(e)).await;
        } else {
            self.send_to_supervisor(HostUnresolved).await;
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
        addr: Arc<SocketAddr>,
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

    // async fn request_trackers(&self) -> bool {
    //     for url in &self.urls {
    //         println!("== URL => {:?}", url);
    //         match url.scheme() {
    //             "http" => {
    //                 println!("== GOING HTTP");
    //                 match self.announce(&self.metadata, url) {
    //                     Ok(addrs) => {
    //                         self.supervisor.send(TorrentNotification::PeerDiscovered {
    //                             addrs
    //                         }).await;
    //                         return true;
    //                     }
    //                     Err(e) => {
    //                         eprintln!("HTTP ERROR {:?}", e);
    //                     }
    //                 }
    //             }
    //             "udp" => {
    //                 let mut buffer = Vec::with_capacity(16 * 1024);
    //                 write_message(ConnectRequest {
    //                     protocol_id: 0x41727101980,
    //                     action: Action::Connect,
    //                     transaction_id: 0x4242
    //                 }.into(), &mut buffer);

    //                 println!("UDP TRYING {:?}", url);

    //                 use std::net::UdpSocket;

    //                 let socket = UdpSocket::bind("0.0.0.0:6888").expect("couldn't bind to address");

    //                 let sockaddr = (
    //                     url.host_str().unwrap(),
    //                     url.port().unwrap_or(80)
    //                 ).to_socket_addrs().unwrap();//.collect::<Vec<_>>();

    //                 println!("SOCKADDR={:?}", sockaddr);

    //                 socket.connect((
    //                     url.host_str().unwrap(),
    //                     url.port().unwrap_or(80)
    //                 )).expect("connect function failed");

    //                 println!("CONNECT DONE", );

    //                 socket.send(&buffer).unwrap();

    //                 println!("BUFFER SENT", );

    //                 socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).expect("set_read_timeout call failed");

    //                 match socket.recv(&mut buffer) {
    //                     Ok(read) => {
    //                         let buf = &buffer[..read];
    //                         let resp = read_response(buf);
    //                         println!("RESPONSE {:?}", resp);
    //                     }
    //                     Err(e) => {
    //                         println!("ERROR UDP {:?}", e);
    //                     }
    //                 }
    //             }
    //             _ => {}
    //         }
    //     }
    //     false
    // }

    // pub fn announce(&self, torrent: &Torrent, url: &Url) -> Result<Vec<SocketAddr>> {
    //     let query = AnnounceQuery::from(torrent);
    //     let response = http_client::get(url, query)?;

    //     let peers = get_peers_addrs(&response);

    //     Ok(peers)
    // }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        println!("TRACKER DROPPED !", );
    }
}
