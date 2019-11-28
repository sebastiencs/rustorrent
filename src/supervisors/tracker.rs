use url::Url;
use async_std::sync::{Sender, Receiver, channel};
use async_std::{task, future};

use std::sync::Arc;
use std::time::Duration;

use crate::metadata::Torrent;
use crate::supervisors::torrent::{Result, TorrentNotification};
use crate::errors::TorrentError;
use crate::actors::tracker::Tracker;

pub enum TrackerMessage {
    Found,
    HostUnresolved,
    ErrorOccured(TorrentError)
}

pub struct TrackerData {
    pub metadata: Arc<Torrent>,
    pub supervisor: Sender<TorrentNotification>,
    pub url: Arc<Url>
}

impl From<(&TrackerSupervisor, &Arc<Url>)> for TrackerData {
    fn from((tracker, url): (&TrackerSupervisor, &Arc<Url>)) -> TrackerData {
        TrackerData {
            metadata: Arc::clone(&tracker.metadata),
            supervisor: tracker.supervisor.clone(),
            url: Arc::clone(url),
        }
    }
}

#[derive(Debug)]
pub struct TrackerSupervisor {
    metadata: Arc<Torrent>,
    supervisor: Sender<TorrentNotification>,
    urls: Vec<Vec<Arc<Url>>>,
    recv: Receiver<TrackerMessage>,
    _sender: Sender<TrackerMessage>,
}

impl TrackerSupervisor {
    pub fn new(
        supervisor: Sender<TorrentNotification>,
        metadata: Arc<Torrent>
    ) -> TrackerSupervisor
    {
        let urls = metadata.get_urls_tiers();
        let (_sender, recv) = channel(10);
        TrackerSupervisor { supervisor, metadata, urls, recv, _sender }
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
                    Tracker::new(data, sender).start().await
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

impl Drop for TrackerSupervisor {
    fn drop(&mut self) {
        println!("TRACKER DROPPED !", );
    }
}
