
use url::Url;
use async_std::sync::Sender;
use async_std::task;
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
    fn new(data: Arc<TrackerData>, addr: Arc<SocketAddr>) -> Box<dyn TrackerConnection + Send + Sync> {
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

struct ATracker {
    data: Arc<TrackerData>,
    addr: Option<Arc<SocketAddr>>,
}

impl ATracker {
    fn new(data: Arc<TrackerData>) -> ATracker {
        ATracker { data, addr: None }
    }

    fn new_with_socket(data: Arc<TrackerData>, addr: Arc<SocketAddr>) -> ATracker {
        ATracker { data, addr: Some(addr) }
    }

    async fn start(&mut self) {
        let addr = self.addr.take().expect("Tracker::start() called without resolving url");
        let data = Arc::clone(&self.data);

        let connection = Self::new_connection(data, addr);
        println!("CONNECTING TO {:?} {:?}", self.data.url, self.addr);
        let peer_addrs = match connection.announce().await {
            Ok(addrs) => addrs,
            Err(e) => {
                println!("ANNOUNCE FAILED {:?}", e);
                return;
            }
        };
        println!("PEERS FOUND ! {:?}\nLENGTH = {:?}", peer_addrs, peer_addrs.len());
        // self.data.supervisor.send(TorrentNotification::PeerDiscovered {
        //     addrs
        // }).await;
    }

    async fn resolve_and_start(&mut self) {
        let addrs = self.resolve_url().await;
        let addrs_len = addrs.len();

        if addrs_len == 0 {
            // Resolving url was unsucessfull
        } else if addrs_len == 1 {
            // 1 address is resolved
            // Let's connect to the tracker in this task
            self.addr = addrs.get(0).cloned();
            println!("SELF_ADDR = {:?}", self.addr);
            self.start().await;
        } else {
            // Multiple addresses are resolved
            // We spawn a task for each ip as some ip might be unresponsive
            for addr in &addrs {
                let addr = Arc::clone(addr);
                let data = Arc::clone(&self.data);
                task::spawn(async move {
                    println!("SPAWN NEW TASK WITH {:?}", addr);
                    Self::new_with_socket(data, addr).start().await;
                });
            }
        }
    }

    fn is_scheme_supported(url: &Url) -> bool {
        match url.scheme() {
            "http" | "udp" => true,
            _ => false
        }
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

    async fn resolve_url(&mut self) -> Vec<Arc<SocketAddr>> {
        let host = self.data.url.host_str().unwrap();
        let port = self.data.url.port().unwrap_or(80);

        (host, port)
            .to_socket_addrs()
            .await
            .map(|s| s.map(Arc::new).collect::<Vec<_>>())
            .unwrap_or_else(|_| vec![])
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
    urls: Vec<Vec<Arc<Url>>>
}

impl Tracker {
    pub fn new(
        supervisor: Sender<TorrentNotification>,
        metadata: Arc<Torrent>
    ) -> Tracker
    {
        let urls = metadata.get_urls_tiers();
        Tracker { supervisor, metadata, urls }
    }

    fn is_scheme_supported(url: &&Arc<Url>) -> bool {
        match url.scheme() {
            "http" | "udp" => true,
            _ => false
        }
    }

    pub async fn start(self) {
        loop {

            // TODO: Check other tiers
            if let Some(tier) = self.urls.first() {
                for url in tier.iter().filter(Self::is_scheme_supported) {

                    let data = Arc::new(TrackerData::from((&self, url)));
                    task::spawn(async move {
                        ATracker::new(data).resolve_and_start().await
                    });
                }
            }

            return;

            // let found = self.request_trackers().await;

            // task::sleep(Duration::from_secs(if found { 90 } else { 5 })).await;
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
