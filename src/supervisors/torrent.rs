use async_channel::{bounded, Receiver, Sender};
use crossbeam_channel::Sender as SyncSender;
use std::sync::Arc;
// use log::info;
use kv_log_macro::{debug, info, warn};

use std::net::SocketAddr;

use crate::{
    actors::{
        peer::{Peer, PeerCommand, PeerExternId, PeerId, PeerTask},
        sha1::Sha1Task,
    },
    bitfield::{BitField, BitFieldUpdate},
    errors::TorrentError,
    metadata::Torrent,
    piece_collector::{Block, PieceCollector},
    piece_picker::PiecePicker,
    pieces::Pieces,
    supervisors::tracker::TrackerSupervisor,
    utils::Map,
};

struct PeerState {
    socket: SocketAddr,
    bitfield: BitField,
    queue_tasks: PeerTask,
    addr: Sender<PeerCommand>,
    extern_id: Arc<PeerExternId>,
    tasks_nbytes: usize,
}

/// Message sent to TorrentSupervisor
pub enum TorrentNotification {
    /// When a Peer is connected, it send this message to be added
    /// to the list of peers
    AddPeer {
        id: PeerId,
        queue: PeerTask,
        addr: Sender<PeerCommand>,
        socket: SocketAddr,
        extern_id: Arc<PeerExternId>,
    },
    /// Message sent when a peer is destroyed (deconnected, ..)
    /// The peer is then removed to the list of peers
    RemovePeer { id: PeerId },
    // /// Message sent when a Peer downloaded a full piece
    // AddPiece { id: PeerId, piece: PieceBuffer },
    /// Message sent when a Peer downloaded a block
    AddBlock { id: PeerId, block: Block },
    /// Update the bitfield of a Peer.
    /// It is sent when the Peer received a BITFIELD or HAVE message
    UpdateBitfield { id: PeerId, update: BitFieldUpdate },
    /// Whether or not the piece match its sha1 sum
    ResultChecksum {
        /// This id is a slab id
        id: usize,
        valid: bool,
    },
    /// When a tracker discover peers, it send this message
    PeerDiscovered { addrs: Vec<SocketAddr> },
}

impl std::fmt::Debug for TorrentNotification {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        use TorrentNotification::*;
        match self {
            AddPeer { id, .. } => f
                .debug_struct("TorrentNotification")
                .field("AddPeer", &id)
                .finish(),
            RemovePeer { id } => f
                .debug_struct("TorrentNotification")
                .field("RemovePeer", &id)
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
        }
    }
}

pub struct TorrentSupervisor {
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
}

pub type Result<T> = std::result::Result<T, TorrentError>;

impl TorrentSupervisor {
    pub fn new(torrent: Torrent, sha1_workers: SyncSender<Sha1Task>) -> TorrentSupervisor {
        let (my_addr, receiver) = bounded(100);
        let pieces_infos = Arc::new(Pieces::from(&torrent));

        let extern_id = Arc::new(PeerExternId::generate());

        let collector = PieceCollector::new(&pieces_infos);
        let piece_picker = PiecePicker::new(&pieces_infos);

        TorrentSupervisor {
            metadata: Arc::new(torrent),
            receiver,
            my_addr,
            collector,
            pieces_infos,
            piece_picker,
            peers: Map::default(),
            sha1_workers,
            extern_id,
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

        self.process_cmds().await;
    }

    fn connect_to_peers(&self, addr: &SocketAddr) {
        debug!("Connecting", { addr: addr.to_string() });

        let addr = *addr;
        let my_addr = self.my_addr.clone();
        let pieces_infos = self.pieces_infos.clone();
        let extern_id = self.extern_id.clone();

        tokio::spawn(async move {
            let mut peer = match Peer::new(addr, pieces_infos, my_addr, extern_id).await {
                Ok(peer) => peer,
                Err(e) => {
                    warn!("Peer error {:?}", e, { addr: addr.to_string() });
                    return;
                }
            };
            let result = peer.start().await;
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
                        peer.bitfield.update(update);

                        if !peer.queue_tasks.is_empty() {
                            continue;
                        }

                        let tasks_nbytes = peer.tasks_nbytes;

                        self.piece_picker.pick_piece(
                            id,
                            tasks_nbytes,
                            &peer.bitfield,
                            &self.collector,
                        );

                        // TODO: Send to Peer

                        // if let Some(piece_index) = self.piece_picker.pick_piece(id, &peer.bitfield)
                        // {
                        //     let nblock_piece = self.pieces_infos.nblocks_piece;
                        //     let block_size = self.pieces_infos.block_size;

                        //     for i in 0..nblock_piece {
                        //         peer.queue_tasks
                        //             .push(BlockToDownload::new(
                        //                 piece_index,
                        //                 BlockIndex::from(i as u32 * block_size),
                        //                 block_size,
                        //             ))
                        //             .unwrap();
                        //     }

                        //     peer.addr.send(PeerCommand::TasksAvailables).await.ok();
                        // }
                    };
                }
                RemovePeer { id } => {
                    self.peers.remove(&id);

                    self.piece_picker.remove_peer(id);
                }
                AddPeer {
                    id,
                    queue,
                    addr,
                    socket,
                    extern_id,
                } => {
                    if self.is_duplicate_peer(&extern_id) {
                        // We are already connected to this peer, disconnect.
                        // This happens when we are connected to its ipv4 and ipv6 addresses

                        addr.send(PeerCommand::Die).await.unwrap();
                    } else {
                        self.peers.insert(
                            id,
                            PeerState {
                                bitfield: BitField::new(self.pieces_infos.num_pieces),
                                queue_tasks: queue,
                                addr,
                                socket,
                                extern_id,
                                tasks_nbytes: self.pieces_infos.piece_length,
                            },
                        );
                    }
                }
                AddBlock { id, block } => {
                    let piece_index = block.piece_index;

                    if let Some(_piece) = self.collector.add_block(&block) {
                        info!("[{}] Piece completed {:?}", id, piece_index);
                        self.piece_picker.set_as_downloaded(piece_index);
                    }

                    if let Some(peer) = self.peers.get_mut(&id) {
                        let tasks_nbytes = peer.tasks_nbytes;

                        self.piece_picker.pick_piece(
                            id,
                            tasks_nbytes,
                            &peer.bitfield,
                            &self.collector,
                        );

                        // TODO: Send to Peer

                        // if peer.queue_tasks.len() <= 5 {
                        //     if let Some(piece_index) =
                        //         self.piece_picker.pick_piece(id, &peer.bitfield)
                        //     {
                        //         let nblock_piece = self.pieces_infos.nblocks_piece;
                        //         let block_size = self.pieces_infos.block_size;

                        //         for i in 0..nblock_piece {
                        //             peer.queue_tasks
                        //                 .push(BlockToDownload::new(
                        //                     piece_index,
                        //                     BlockIndex::from(i as u32 * block_size),
                        //                     block_size,
                        //                 ))
                        //                 .unwrap();
                        //         }

                        //         peer.addr.send(PeerCommand::TasksAvailables).await.ok();
                        //     }
                        // }
                    }
                }
                ResultChecksum { id: _, valid } => {
                    // if self.pending_pieces.contains(id) {
                    //     let _piece = self.pending_pieces.remove(id);
                    // };
                    debug!("Piece checked from the pool: {}", valid);
                }
                PeerDiscovered { addrs } => {
                    for addr in &addrs {
                        let mut peers = self.peers.values();
                        if !peers.any(|p| &p.socket == addr) {
                            self.connect_to_peers(addr);
                        } else {
                            warn!("Already connected to {:?}, ignoring", addr);
                        }
                    }
                } // AddPiece { id, piece } => {
                  //     let index = piece.piece_index;
                  //     if let Some(sum_metadata) =
                  //         self.pieces_infos.sha1_pieces.get(index).map(Arc::clone)
                  //     {
                  //         let piece_buffer = Arc::new(piece);
                  //         let addr = self.my_addr.clone();
                  //         let id = self.pending_pieces.insert(Arc::clone(&piece_buffer));
                  //         self.sha1_workers
                  //             .send(Sha1Task::CheckSum {
                  //                 piece_buffer,
                  //                 sum_metadata,
                  //                 id,
                  //                 addr,
                  //             })
                  //             .unwrap();
                  //     }
                  // }
            }
        }
    }

    /// Check if the peer extern id is already in our state
    fn is_duplicate_peer(&self, id: &PeerExternId) -> bool {
        self.peers.values().any(|p| &*p.extern_id == id)
    }
}
