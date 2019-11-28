
mod udp;
pub mod http;

use async_std::sync::Sender;
use async_std::task;
use async_std::net::{SocketAddr, ToSocketAddrs};
use async_trait::async_trait;

use std::sync::Arc;
use std::time::Duration;

use crate::supervisors::torrent::{Result, TorrentNotification};
use crate::errors::TorrentError;
use crate::supervisors::tracker::{TrackerData, TrackerMessage};
use crate::metadata::UrlHash;

#[async_trait]
trait TrackerConnection {
    async fn announce(&mut self, connected_addr: &mut usize) -> Result<Vec<SocketAddr>>;
    async fn scrape(&mut self) -> Result<()>;
}

pub struct Tracker {
    data: Arc<TrackerData>,
    /// All addresses the host were resolved to
    /// When we're connected to an address, it is moved to the first position
    /// so later requests will use this address first.
    addrs: Vec<Arc<SocketAddr>>,
    tracker_supervisor: Sender<(UrlHash, TrackerMessage)>,
}

impl Tracker {
    pub fn new(data: Arc<TrackerData>, tracker_supervisor: Sender<(UrlHash, TrackerMessage)>) -> Tracker {
        Tracker { data, addrs: Vec::new(), tracker_supervisor }
    }

    pub async fn start(&mut self) {
        loop {
            self.resolve_and_start().await;

            // TODO: Use interval from announce response
            task::sleep(Duration::from_secs(120)).await;
        }
    }

    fn set_connected_addr(&mut self, index: usize) {
        if index != 0 {
            self.addrs.swap(0, index);
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

    async fn send_to_supervisor(&self, msg: TrackerMessage) {
        self.tracker_supervisor.send((self.data.url.hash(), msg)).await;
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
            "http" => http::HttpConnection::new(data, addr),
            "udp" => udp::UdpConnection::new(data, addr),
            _ => unreachable!()
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

impl Drop for Tracker {
    fn drop(&mut self) {
        println!("ATRACKER DROPPED !", );
    }
}
