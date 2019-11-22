
use url::Url;
use async_std::sync::Sender;
use async_std::task;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::metadata::Torrent;
use crate::supervisors::torrent::{Result, TorrentNotification};
use crate::http_client::{self, AnnounceQuery, AnnounceResponse};
use crate::session::get_peers_addrs;

#[derive(Debug)]
pub struct Tracker {
    metadata: Arc<Torrent>,
    supervisor: Sender<TorrentNotification>,
    urls: Vec<Url>
}

impl Tracker {
    pub fn new(
        supervisor: Sender<TorrentNotification>,
        metadata: Arc<Torrent>
    ) -> Tracker {
        let urls = metadata.iter_urls().collect();
        Tracker { supervisor, metadata, urls }
    }
    
    pub async fn start(self) {
        loop {
            let found = self.request_trackers().await;
            
            task::sleep(Duration::from_secs(if found { 90 } else { 5 })).await;
        }
    }

    async fn request_trackers(&self) -> bool {
        for url in &self.urls {
            if let Ok(addrs) = self.announce(&self.metadata, url) {
                self.supervisor.send(TorrentNotification::PeerDiscovered {
                    addrs
                }).await;
                return true;
            };
        }
        false
    }

    pub fn announce(&self, torrent: &Torrent, url: &Url) -> Result<Vec<SocketAddr>> {
        let query = AnnounceQuery::from(torrent);
        let response = http_client::get(url, query)?;

        let peers = get_peers_addrs(&response);

        Ok(peers)
    }
}
