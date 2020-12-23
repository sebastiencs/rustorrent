use async_channel::{bounded, Receiver, Sender};
use crossbeam_channel::Sender as SyncSender;
use std::sync::{
    atomic::{
        AtomicUsize,
        Ordering::{self, Acquire, Relaxed},
    },
    Arc,
};
// use log::info;
use kv_log_macro::{debug, info, warn};

use std::net::SocketAddr;

use crate::{
    actors::{
        peer::{Peer, PeerCommand, PeerExternId, PeerId},
        sha1::Sha1Task,
        vfs::VFSMessage,
    },
    bitfield::{BitField, BitFieldUpdate},
    errors::TorrentError,
    metadata::Torrent,
    piece_collector::{Block, PieceCollector},
    piece_picker::{PieceIndex, PiecePicker},
    pieces::{Pieces, TaskDownload},
    spsc::{self, Producer},
    supervisors::tracker::TrackerSupervisor,
    utils::Map,
};

static TORRENT_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Eq, PartialEq, Copy, Clone, Debug, Hash)]
pub struct TorrentId(usize);

impl std::fmt::Display for TorrentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Torrent {}", self.0)
    }
}

impl TorrentId {
    fn new() -> Self {
        let id = TORRENT_COUNTER.fetch_add(1, Ordering::SeqCst);
        Self(id)
    }
}

pub struct Shared {
    pub nbytes_on_tasks: AtomicUsize,
}

impl Shared {
    pub fn new() -> Self {
        Shared {
            nbytes_on_tasks: AtomicUsize::new(0),
        }
    }
}

impl Default for Shared {
    fn default() -> Self {
        Shared::new()
    }
}

struct PeerState {
    socket: SocketAddr,
    bitfield: BitField,
    queue_tasks: Producer<TaskDownload>,
    addr: Sender<PeerCommand>,
    extern_id: Arc<PeerExternId>,
    tasks_nbytes: usize,
    shared: Arc<Shared>,
}

pub struct NewPeer {
    pub id: PeerId,
    pub queue: Producer<TaskDownload>,
    pub addr: Sender<PeerCommand>,
    pub socket: SocketAddr,
    pub extern_id: Arc<PeerExternId>,
    pub shared: Arc<Shared>,
}

/// Message sent to TorrentSupervisor
pub enum TorrentNotification {
    AddPeer {
        peer: Box<NewPeer>,
    },
    /// Message sent when a peer is destroyed (deconnected, ..)
    /// The peer is then removed to the list of peers
    RemovePeer {
        id: PeerId,
    },
    IncreaseTasksPeer {
        id: PeerId,
    },
    // /// Message sent when a Peer downloaded a full piece
    // AddPiece { id: PeerId, piece: PieceBuffer },
    /// Message sent when a Peer downloaded a block
    AddBlock {
        id: PeerId,
        block: Block,
    },
    /// Update the bitfield of a Peer.
    /// It is sent when the Peer received a BITFIELD or HAVE message
    UpdateBitfield {
        id: PeerId,
        update: Box<BitFieldUpdate>,
    },
    /// Whether or not the piece match its sha1 sum
    ResultChecksum {
        /// This id is a slab id
        id: usize,
        valid: bool,
    },
    /// When a tracker discover peers, it send this message
    PeerDiscovered {
        addrs: Box<[SocketAddr]>,
    },
    PieceFromVFS {
        piece: PieceIndex,
        data: Box<[u8]>,
    },
}

impl std::fmt::Debug for TorrentNotification {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        use TorrentNotification::*;
        match self {
            AddPeer { peer } => f
                .debug_struct("TorrentNotification")
                .field("AddPeer", &peer.id)
                .finish(),
            RemovePeer { id } => f
                .debug_struct("TorrentNotification")
                .field("RemovePeer", &id)
                .finish(),
            IncreaseTasksPeer { id } => f
                .debug_struct("TorrentNotification")
                .field("IncreaseTasksPeer", &id)
                .finish(),
            AddBlock { id: _, block } => f
                .debug_struct("TorrentNotification")
                .field("AddBlock", &block)
                .finish(),
            UpdateBitfield { id, .. } => f
                .debug_struct("TorrentNotification")
                .field("UpdateBitfield", &id)
                .finish(),
            ResultChecksum { id, valid } => f
                .debug_struct("TorrentNotification")
                .field("ResultChecksum", &id)
                .field("valid", &valid)
                .finish(),
            PeerDiscovered { addrs } => f
                .debug_struct("TorrentNotification")
                .field("addrs", &addrs)
                .finish(),
            TorrentNotification::PieceFromVFS { piece, .. } => f
                .debug_struct("TorrentNotification")
                .field("PieceFromVFS", &piece)
                .finish(),
        }
    }
}

pub struct TorrentSupervisor {
    id: TorrentId,

    metadata: Arc<Torrent>,
    receiver: Receiver<TorrentNotification>,
    // We keep a Sender to not close the channel
    // in case there is no peer
    my_addr: Sender<TorrentNotification>,

    pieces_infos: Arc<Pieces>,

    peers: Map<PeerId, PeerState>,

    piece_picker: PiecePicker,

    collector: PieceCollector,

    sha1_workers: SyncSender<Sha1Task>,

    extern_id: Arc<PeerExternId>,

    vfs: Sender<VFSMessage>,
}

pub type Result<T> = std::result::Result<T, TorrentError>;

impl TorrentSupervisor {
    pub fn new(
        torrent: Torrent,
        sha1_workers: SyncSender<Sha1Task>,
        vfs: Sender<VFSMessage>,
    ) -> TorrentSupervisor {
        let (my_addr, receiver) = bounded(10000);
        let pieces_infos = Arc::new(Pieces::from(&torrent));

        let extern_id = Arc::new(PeerExternId::generate());

        let collector = PieceCollector::new(&pieces_infos);
        let piece_picker = PiecePicker::new(&pieces_infos);

        let id = TorrentId::new();

        TorrentSupervisor {
            id,
            metadata: Arc::new(torrent),
            receiver,
            my_addr,
            collector,
            pieces_infos,
            piece_picker,
            peers: Map::default(),
            sha1_workers,
            extern_id,
            vfs,
        }
    }

    pub async fn start(&mut self) {
        let metadata = Arc::clone(&self.metadata);
        let my_addr = self.my_addr.clone();
        let extern_id = self.extern_id.clone();

        tokio::spawn(async {
            TrackerSupervisor::new(my_addr, metadata, extern_id)
                .start()
                .await;
        });

        self.vfs
            .send(VFSMessage::AddTorrent {
                id: self.id,
                meta: Arc::clone(&self.metadata),
                pieces_infos: Arc::clone(&self.pieces_infos),
            })
            .await
            .unwrap();

        self.process_cmds().await;
    }

    fn connect_to_peers(&self, addr: &SocketAddr) {
        debug!("Connecting", { addr: addr.to_string() });

        let addr = *addr;
        let my_addr = self.my_addr.clone();
        let pieces_infos = self.pieces_infos.clone();
        let extern_id = self.extern_id.clone();

        tokio::spawn(async move {
            let (producer, consumer) = spsc::bounded(256);

            let mut peer = match Peer::new(addr, pieces_infos, my_addr, extern_id, consumer).await {
                Ok(peer) => peer,
                Err(e) => {
                    warn!("Peer error {:?}", e, { addr: addr.to_string() });
                    return;
                }
            };
            let result = peer.start(producer).await;
            warn!("[{}] Peer terminated: {:?}", peer.internal_id(), result, { addr: addr.to_string() });
        });
    }

    async fn process_cmds(&mut self) {
        use TorrentNotification::*;

        while let Ok(msg) = self.receiver.recv().await {
            match msg {
                UpdateBitfield { id, update } => {
                    if let Some(peer) = self.peers.get_mut(&id) {
                        self.piece_picker.update(&update);
                        peer.bitfield.update(*update);

                        if !peer.queue_tasks.is_empty() {
                            continue;
                        }

                        let tasks_nbytes = peer.tasks_nbytes;
                        let available = peer.queue_tasks.available();

                        if let Some((nbytes, tasks)) = self.piece_picker.pick_piece(
                            id,
                            tasks_nbytes,
                            available,
                            &peer.bitfield,
                            &self.collector,
                        ) {
                            warn!("[{}] Tasks found {:?}", id, tasks);
                            peer.shared.nbytes_on_tasks.fetch_add(nbytes, Relaxed);
                            peer.queue_tasks.push_slice(tasks).unwrap();
                        } else {
                            warn!("[{}] Tasks not found", id);
                        }

                        peer.addr.send(PeerCommand::TasksAvailables).await.ok();
                    };
                }
                RemovePeer { id } => {
                    self.peers.remove(&id);
                    self.piece_picker.remove_peer(id);
                }
                IncreaseTasksPeer { id } => {
                    if let Some(peer) = self.peers.get_mut(&id) {
                        if self
                            .piece_picker
                            .would_pick_piece(id, &peer.bitfield, &self.collector)
                        {
                            info!("[{}] Multiply tasks {:?}", id, peer.tasks_nbytes * 3);

                            peer.tasks_nbytes = peer.tasks_nbytes.saturating_mul(3);
                            peer.addr.send(PeerCommand::TasksIncreased).await.ok();
                        } else {
                            info!("[{}] No more piece available for this peer", id);
                        }
                    }
                }
                AddPeer { peer } => {
                    if self.is_duplicate_peer(&peer.extern_id) {
                        // We are already connected to this peer, disconnect.
                        // This happens when we are connected to its ipv4 and ipv6 addresses

                        peer.addr.send(PeerCommand::Die).await.ok();
                    } else {
                        self.peers.insert(
                            peer.id,
                            PeerState {
                                bitfield: BitField::new(self.pieces_infos.num_pieces),
                                queue_tasks: peer.queue,
                                addr: peer.addr,
                                socket: peer.socket,
                                extern_id: peer.extern_id,
                                shared: peer.shared,
                                tasks_nbytes: self.pieces_infos.piece_length,
                            },
                        );
                    }
                }
                AddBlock { id, block } => {
                    let piece_index = block.piece_index;

                    if let Some(piece) = self.collector.add_block(&block) {
                        info!("[{}] Piece completed {:?}", id, piece_index);
                        self.piece_picker.set_as_downloaded(piece_index);

                        self.vfs
                            .send(VFSMessage::Write {
                                id: self.id,
                                piece: piece_index,
                                data: piece,
                            })
                            .await
                            .unwrap();
                    }

                    if let Some(peer) = self.peers.get_mut(&id) {
                        let tasks_nbytes = peer.tasks_nbytes;

                        if peer.shared.nbytes_on_tasks.load(Acquire) < tasks_nbytes / 2 {
                            let available = peer.queue_tasks.available().saturating_sub(1);

                            if let Some((nbytes, tasks)) = self.piece_picker.pick_piece(
                                id,
                                tasks_nbytes,
                                available,
                                &peer.bitfield,
                                &self.collector,
                            ) {
                                info!("[{}] Adding {} tasks {:?}", id, tasks.len(), tasks);
                                peer.shared.nbytes_on_tasks.fetch_add(nbytes, Relaxed);
                                peer.queue_tasks.push_slice(tasks).unwrap();

                                peer.addr.send(PeerCommand::TasksAvailables).await.ok();
                            }
                        }
                    }
                }
                ResultChecksum { id: _, valid } => {
                    // if self.pending_pieces.contains(id) {
                    //     let _piece = self.pending_pieces.remove(id);
                    // };
                    debug!("Piece checked from the pool: {}", valid);
                }
                PeerDiscovered { addrs } => {
                    for addr in &*addrs {
                        let mut peers = self.peers.values();
                        if !peers.any(|p| &p.socket == addr) {
                            self.connect_to_peers(addr);
                        } else {
                            warn!("Already connected to {:?}, ignoring", addr);
                        }
                    }
                }
                PieceFromVFS { piece: _, data: _ } => {}
            }
        }
    }

    /// Check if the peer extern id is already in our state
    fn is_duplicate_peer(&self, id: &PeerExternId) -> bool {
        self.peers.values().any(|p| &*p.extern_id == id)
    }
}

impl Drop for TorrentSupervisor {
    fn drop(&mut self) {
        self.vfs
            .try_send(VFSMessage::RemoveTorrent { id: self.id })
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::TorrentNotification;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn assert_message_size() {
        assert_eq!(std::mem::size_of::<TorrentNotification>(), 40);
    }
}
