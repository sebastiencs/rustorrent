use async_channel::Sender;
use async_trait::async_trait;
use kv_log_macro::{error, info, warn};

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    errors::TorrentError,
    metadata::UrlHash,
    torrent::{Result, TorrentNotification},
    tracker::supervisor::{TrackerData, TrackerStatus},
};

#[async_trait]
pub trait TrackerConnection {
    async fn announce(&mut self, connected_addr: &mut usize) -> Result<Vec<SocketAddr>>;
    async fn scrape(&mut self) -> Result<()>;
}

pub struct Tracker {
    data: Arc<TrackerData>,
    /// All addresses the host were resolved to
    /// When we're connected to an address, it is moved to the first position
    /// so later requests will use this address first.
    addrs: Vec<Arc<SocketAddr>>,
    tracker_supervisor: Sender<(UrlHash, Instant, TrackerStatus)>,
}

impl Tracker {
    pub fn new(
        data: Arc<TrackerData>,
        tracker_supervisor: Sender<(UrlHash, Instant, TrackerStatus)>,
    ) -> Tracker {
        Tracker {
            data,
            addrs: Vec::new(),
            tracker_supervisor,
        }
    }

    pub async fn start(&mut self) {
        loop {
            self.resolve_and_start().await;

            // TODO: Use interval from announce response
            tokio::time::sleep(Duration::from_secs(120)).await;
        }
    }

    fn set_connected_addr(&mut self, index: usize) {
        if index != 0 {
            self.addrs.swap(0, index);
        }
    }

    async fn resolve_and_start(&mut self) {
        use TrackerStatus::*;

        self.addrs = self.resolve_host().await;

        info!("[tracker] Resolved addresses {:?}", self.addrs);

        if self.addrs.is_empty() {
            self.send_to_supervisor(HostUnresolved).await;
            return;
        }

        match self.connect_and_request().await {
            Ok(peer_addrs) => {
                info!(
                    "[tracker] Peers found {:?}\nLength = {:?}",
                    peer_addrs,
                    peer_addrs.len()
                );
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
            }
            Ok(empty) => Ok(empty),
            Err(e) => {
                error!("[tracker] Announce failed {:?}", e);
                Err(e)
            }
        }
    }

    async fn send_to_supervisor(&self, msg: TrackerStatus) {
        self.tracker_supervisor
            .send((self.data.url.hash(), Instant::now(), msg))
            .await
            .unwrap();
    }

    async fn send_addrs(&self, addrs: Vec<SocketAddr>) {
        use TorrentNotification::PeerDiscovered;
        use TrackerStatus::*;

        self.send_to_supervisor(FoundPeers(addrs.len())).await;
        self.data
            .supervisor
            .send(PeerDiscovered {
                addrs: addrs.into_boxed_slice(),
            })
            .await
            .unwrap();
    }

    fn new_connection(
        data: Arc<TrackerData>,
        addr: Vec<Arc<SocketAddr>>,
    ) -> Box<dyn TrackerConnection + Send + Sync> {
        match data.url.scheme() {
            "http" => super::http::HttpConnection::new(data, addr),
            "udp" => super::udp::UdpConnection::new(data, addr),
            scheme => {
                panic!("Unhandled scheme {:?}", scheme);
            }
        }
    }

    async fn resolve_host(&mut self) -> Vec<Arc<SocketAddr>> {
        let host = self.data.url.host_str().unwrap();
        let port = self.data.url.port().unwrap_or(80);

        tokio::net::lookup_host((host, port))
            .await
            .map(|a| a.map(Arc::new).collect())
            .unwrap_or_else(|_| Vec::new())
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        warn!("[tracker] Dropped: {:?}", self.data.url);
    }
}
