use url::Url;
use async_std::sync::{Sender, Receiver, channel, RwLock};
use async_std::{task, future};

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::metadata::Torrent;
use crate::supervisors::torrent::TorrentNotification;
use crate::errors::TorrentError;
use crate::actors::tracker::Tracker;

#[derive(Debug)]
pub enum TrackerMessage {
    Found,
    HostUnresolved,
    ErrorOccured(TorrentError)
}

pub struct TrackerData {
    pub metadata: Arc<Torrent>,
    pub supervisor: Sender<TorrentNotification>,
    pub url: Arc<TrackerUrl>
}

impl From<(&TrackerSupervisor, &Arc<TrackerUrl>)> for TrackerData {
    fn from((tracker, url): (&TrackerSupervisor, &Arc<TrackerUrl>)) -> TrackerData {
        TrackerData {
            metadata: Arc::clone(&tracker.metadata),
            supervisor: tracker.supervisor.clone(),
            url: Arc::clone(url),
        }
    }
}

#[derive(Debug)]
pub struct TrackerState {
    last_msg: TrackerMessage,
    last_msg_time: Instant
}

impl From<TrackerMessage> for TrackerState {
    fn from(msg: TrackerMessage) -> TrackerState {
        TrackerState {
            last_msg: msg,
            last_msg_time: Instant::now()
        }
    }
}

use crate::metadata::{TrackerUrl, UrlHash};
use crate::utils::Map;

#[derive(Debug)]
pub struct TrackerSupervisor {
    metadata: Arc<Torrent>,
    supervisor: Sender<TorrentNotification>,
    urls: Vec<Vec<Arc<TrackerUrl>>>,
    recv: Receiver<(UrlHash, TrackerMessage)>,
    _sender: Sender<(UrlHash, TrackerMessage)>,

    tracker_states: Arc<RwLock<Map<UrlHash, TrackerState>>>,
}

impl TrackerSupervisor {
    pub fn new(
        supervisor: Sender<TorrentNotification>,
        metadata: Arc<Torrent>
    ) -> TrackerSupervisor
    {
        let urls = metadata.get_urls_tiers();
        let (_sender, recv) = channel(10);
        TrackerSupervisor {
            supervisor,
            metadata,
            urls,
            recv,
            _sender,
            tracker_states: Default::default()
        }
    }

    pub fn is_scheme_supported(url: &Url) -> bool {
        match url.scheme() {
            "http" | "udp" => true,
            _ => false
        }
    }

    pub async fn start(self) {
        self.loop_until_connected().await;
        self.wait_on_tracker_msg().await
    }

    async fn loop_until_connected(&self) {
        for tier in self.urls.as_slice() {
            for url in tier.iter() {
                self.spawn_tracker(url).await;

                let duration = Duration::from_secs(15);
                match future::timeout(duration, self.recv.recv()).await {
                    Ok(Some((url, TrackerMessage::Found))) => {
                        self.update_state(url, TrackerMessage::Found).await;
                        // 1 is connected, stop the loop
                        return;
                    },
                    Ok(Some((url, msg))) => {
                        self.update_state(url, msg).await;
                    }
                    _ => {} // We loop on urls until connected to one
                }
            }
        }
    }

    async fn spawn_tracker(&self, url: &Arc<TrackerUrl>) {
        let data = Arc::new(TrackerData::from((self, url)));
        let sender = self._sender.clone();

        task::spawn(async move {
            Tracker::new(data, sender).start().await
        });
    }

    async fn update_state(&self, url: UrlHash, msg: TrackerMessage) {
        let msg = msg.into();

        self.tracker_states
            .write()
            .await
            .insert(url, msg);
    }

    async fn wait_on_tracker_msg(&self) {
        while let Some((url, msg)) = self.recv.recv().await {
            self.update_state(url, msg).await;

            if !self.is_one_connected().await {
                self.try_another_tracker().await;
            }
        }
    }

    async fn try_another_tracker(&self) {
        let spawned = {
            let states = self.tracker_states.read().await;
            states.keys().copied().collect::<Vec<_>>()
        };

        for tier in self.urls.as_slice() {
            for url in tier.iter() {
                if !spawned.contains(&url.hash()) {
                    self.spawn_tracker(url).await;
                    return;
                }
            }
        }
    }

    async fn is_one_connected(&self) -> bool {
        let states = self.tracker_states.read().await;
        for state in states.values() {
            if let TrackerMessage::Found = state.last_msg {
                return true;
            }
        }
        false
    }
}

impl Drop for TrackerSupervisor {
    fn drop(&mut self) {
        println!("TRACKER DROPPED !", );
    }
}
