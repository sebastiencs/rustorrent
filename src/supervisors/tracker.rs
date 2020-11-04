use url::Url;
use async_channel::{Sender, Receiver, bounded};

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::metadata::Torrent;
use crate::supervisors::torrent::TorrentNotification;
use crate::errors::TorrentError;
use crate::actors::tracker::Tracker;
use crate::actors::peer::PeerExternId;

#[derive(Debug)]
pub enum TrackerStatus {
    FoundPeers(usize),
    HostUnresolved,
    ErrorOccured(TorrentError)
}

pub struct TrackerData {
    pub metadata: Arc<Torrent>,
    pub supervisor: Sender<TorrentNotification>,
    pub url: Arc<TrackerUrl>,
    pub extern_id: Arc<PeerExternId>
}

impl From<(&TrackerSupervisor, &Arc<TrackerUrl>)> for TrackerData {
    fn from((tracker, url): (&TrackerSupervisor, &Arc<TrackerUrl>)) -> TrackerData {
        TrackerData {
            metadata: Arc::clone(&tracker.metadata),
            supervisor: tracker.supervisor.clone(),
            url: Arc::clone(url),
            extern_id: tracker.extern_id.clone()
        }
    }
}

#[derive(Debug)]
pub struct TrackerState {
    last_status: TrackerStatus,
    last_status_time: Instant
}

impl From<(Instant, TrackerStatus)> for TrackerState {
    fn from((instant, msg): (Instant, TrackerStatus)) -> TrackerState {
        TrackerState {
            last_status: msg,
            last_status_time: instant
        }
    }
}

use crate::metadata::{TrackerUrl, UrlHash};
use crate::utils::Map;

#[derive(Debug)]
pub struct TrackerSupervisor {
    metadata: Arc<Torrent>,
    /// [`TorrentSupervisor`]
    supervisor: Sender<TorrentNotification>,
    /// List of urls, by tier
    urls: Vec<Arc<TrackerUrl>>,
    recv: Receiver<(UrlHash, Instant, TrackerStatus)>,
    /// Keep a sender to not close the channel
    _sender: Sender<(UrlHash, Instant, TrackerStatus)>,
    /// Urls are already hashed so we can move it everywhere just by copy
    /// Otherwise we would have to clone an Arc<Url> in every messages etc.
    tracker_states: Map<UrlHash, TrackerState>,
    /// Our peer_id we send to trackers
    extern_id: Arc<PeerExternId>
}

impl TrackerSupervisor {
    pub fn new(
        supervisor: Sender<TorrentNotification>,
        metadata: Arc<Torrent>,
        extern_id: Arc<PeerExternId>,
    ) -> TrackerSupervisor
    {
        let urls = metadata.get_urls_tiers();
        let (_sender, recv) = bounded(10);
        TrackerSupervisor {
            supervisor,
            metadata,
            urls,
            recv,
            _sender,
            extern_id,
            tracker_states: Default::default()
        }
    }

    pub fn is_scheme_supported(url: &Url) -> bool {
        matches!(url.scheme(), "http" | "udp")
    }

    pub async fn start(mut self) {
        self.loop_until_connected().await;
        self.wait_on_tracker_msg().await
    }

    async fn loop_until_connected(&mut self) {
        let mut pending_status = Vec::with_capacity(10);

        for url in self.urls.as_slice() {
            self.spawn_tracker(url).await;

            // We wait 15 secs, if we aren't connected to this tracker
            // we spawn another actor
            let duration = Duration::from_secs(15);
            match tokio::time::timeout(duration, self.recv.recv()).await {
                Ok(Ok((url, instant, TrackerStatus::FoundPeers(n)))) => {
                    pending_status.push((url, instant, TrackerStatus::FoundPeers(n)));
                    // 1 is connected, stop the loop
                    break;
                },
                Ok(Ok((url, instant, msg))) => {
                    pending_status.push((url, instant, msg));
                }
                _ => {} // We loop on urls until connected to one
            }
        }

        // We update the state outside the loop to make the
        // borrow checker happy
        for (url, instant, status) in pending_status {
            self.update_state(url, instant, status)
        }
    }

    async fn spawn_tracker(&self, url: &Arc<TrackerUrl>) {
        let data = Arc::new(TrackerData::from((self, url)));
        let sender = self._sender.clone();

        tokio::spawn(async move {
            Tracker::new(data, sender).start().await
        });
    }

    fn update_state(&mut self, url: UrlHash, instant: Instant, msg: TrackerStatus) {
        let msg = (instant, msg).into();

        self.tracker_states
            .insert(url, msg);
    }

    async fn wait_on_tracker_msg(&mut self) {
        while let Ok((url, instant, msg)) = self.recv.recv().await {
            self.update_state(url, instant, msg);

            if !self.is_one_active() {
                self.try_another_tracker().await;
            }
        }
    }

    async fn try_another_tracker(&self) {
        let spawned = {
            self.tracker_states.keys().copied().collect::<Vec<_>>()
        };

        for url in self.urls.as_slice() {
            if !spawned.contains(&url.hash()) {
                self.spawn_tracker(url).await;
                return;
            }
        }
    }

    fn is_one_active(&self) -> bool {
        for state in self.tracker_states.values() {
            if let TrackerStatus::FoundPeers(n) = state.last_status {
                // We consider the tracker active if it found at least 2
                // peer addresses
                if n > 1 {
                    return true;
                }
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
