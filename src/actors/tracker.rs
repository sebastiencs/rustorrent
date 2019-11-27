
use url::Url;
use async_std::sync::{Sender, Receiver, channel};
use async_std::{task, future};
use async_std::net::{SocketAddr, ToSocketAddrs};
use async_trait::async_trait;

use std::sync::Arc;
use std::time::Duration;

use crate::metadata::Torrent;
use crate::supervisors::torrent::{Result, TorrentNotification};
use crate::http_client::{self, AnnounceQuery, AnnounceResponse};
use crate::session::get_peers_addrs;
use crate::actors::tracker_udp::{write_message, read_response, Action, ConnectRequest};

#[async_trait]
trait TrackerConnection {
    async fn announce(&self) -> Result<Vec<SocketAddr>>;
}

struct UdpConnection {
    data: Arc<TrackerData>,
    addr: Arc<SocketAddr>
}

struct HttpConnection {
    data: Arc<TrackerData>,
    addr: Arc<SocketAddr>,
}

#[async_trait]
impl TrackerConnection for UdpConnection {
    async fn announce(&self) -> Result<Vec<SocketAddr>> { Ok(vec![]) }
}

#[async_trait]
impl TrackerConnection for HttpConnection {
    async fn announce(&self) -> Result<Vec<SocketAddr>> {
        let query = AnnounceQuery::from(self.data.metadata.as_ref());
        let response = http_client::get(&self.data.url, query, self.addr.as_ref()).await?;

        let peers = get_peers_addrs(&response);

        Ok(peers)
    }
}

impl UdpConnection {
    fn new(
        data: Arc<TrackerData>,
        addr: Arc<SocketAddr>
    ) -> Box<dyn TrackerConnection + Send + Sync>
    {
        Box::new(Self { data, addr })
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
    NotResponding
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
            let connection = Self::new_connection(data, Arc::clone(addr));

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
            self.send_to_supervisor(NotResponding).await;
        } else {
            self.send_to_supervisor(HostUnresolved).await;
        }
    }

    async fn send_to_supervisor(&self, msg: TrackerMessage) {
        self.tracker_supervisor.send(msg).await;
    }

    async fn send_addrs(&self, addrs: Vec<SocketAddr>) {
        use TorrentNotification::PeerDiscovered;

        // self.data.supervisor.send(PeerDiscovered { addrs }).await;
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

struct TrackerData {
    metadata: Arc<Torrent>,
    supervisor: Sender<TorrentNotification>,
    url: Arc<Url>
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

        loop {

            // TODO: Check other tiers
            if let Some(tier) = self.urls.first() {
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
            return;
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
