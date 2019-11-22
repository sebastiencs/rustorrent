
use std::sync::Arc;
use parking_lot::{RwLock, RwLockReadGuard};
use async_std::prelude::*;
use async_std::task;
use async_std::sync as a_sync;
use sha1::Sha1;
use crossbeam_channel::Sender;
use slab::Slab;

use std::net::{SocketAddr, ToSocketAddrs};

use crate::actors::peer::{PeerId, Peer, PeerTask, PeerCommand};
use crate::metadata::Torrent;
use crate::bitfield::{BitFieldUpdate, BitField};
use crate::pieces::{PieceInfo, Pieces, PieceBuffer, PieceToDownload};
use crate::actors::tracker::Tracker;
use crate::utils::Map;
use crate::errors::TorrentError;
use crate::actors::sha1::{Sha1Workers, Sha1Task};

struct PeerState {
    socket: SocketAddr,
    bitfield: BitField,
    queue_tasks: PeerTask,
    addr: a_sync::Sender<PeerCommand>
}

/// Message sent to Supervisor
pub enum TorrentNotification {
    AddPeer {
        id: PeerId,
        queue: PeerTask,
        addr: a_sync::Sender<PeerCommand>,
        socket: SocketAddr,
    },
    RemovePeer {
        id: PeerId ,
        queue: PeerTask
    },
    AddPiece(PieceBuffer),
    UpdateBitfield {
        id: PeerId,
        update: BitFieldUpdate
    },
    ResultChecksum {
        id: usize,
        valid: bool
    },
    PeerDiscovered {
        addrs: Vec<SocketAddr>
    }
}

impl std::fmt::Debug for TorrentNotification {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        use TorrentNotification::*;
        match self {
            AddPeer { id, .. } => {
                f.debug_struct("TorrentNotification")
                 .field("AddPeer", &id)
                 .finish()
            }
            RemovePeer { id, .. } => {
                f.debug_struct("TorrentNotification")
                 .field("RemovePeer", &id)
                 .finish()
            }
            AddPiece(piece) => {
                f.debug_struct("TorrentNotification")
                 .field("AddPiece", &piece.piece_index)
                 .finish()
            }
            UpdateBitfield { id, .. } => {
                f.debug_struct("TorrentNotification")
                 .field("UpdateBitfield", &id)
                 .finish()
            }
            ResultChecksum { id, valid } => {
                f.debug_struct("TorrentNotification")
                 .field("ResultChecksum", &id)
                 .field("valid", &valid)
                 .finish()
            }
            PeerDiscovered { addrs } => {
                f.debug_struct("TorrentNotification")
                 .field("addrs", &addrs)
                 .finish()
            }
        }
    }
}

pub struct TorrentSupervisor {
    metadata: Arc<Torrent>,
    receiver: a_sync::Receiver<TorrentNotification>,
    // We keep a Sender to not close the channel
    // in case there is no peer
    my_addr: a_sync::Sender<TorrentNotification>,

    pieces_detail: Pieces,

    peers: Map<PeerId, PeerState>,

    pieces: Vec<Option<PieceInfo>>,

    sha1_workers: Sender<Sha1Task>,

    pending_pieces: Slab<Arc<PieceBuffer>>
}

pub type Result<T> = std::result::Result<T, TorrentError>;

impl TorrentSupervisor {
    pub fn new(torrent: Torrent, sha1_workers: Sender<Sha1Task>) -> TorrentSupervisor {
        let (my_addr, receiver) = a_sync::channel(100);
        let pieces_detail = Pieces::from(&torrent);

        let num_pieces = pieces_detail.num_pieces;
        let mut pieces = Vec::with_capacity(num_pieces);
        pieces.resize_with(num_pieces, Default::default);

        TorrentSupervisor {
            metadata: Arc::new(torrent),
            receiver,
            my_addr,
            pieces_detail,
            pieces,
            peers: Default::default(),
            sha1_workers,
            pending_pieces: Slab::new(),
        }
    }

    pub async fn start(&mut self) {
        let metadata = Arc::clone(&self.metadata);
        let my_addr = self.my_addr.clone();

        task::spawn(async {
            Tracker::new(my_addr, metadata).start().await;
        });

        self.process_cmds().await;
    }

    fn connect_to_peers(&self, addr: &SocketAddr) {
        println!("CONNECTING TO: {:?}", addr);

        let addr = *addr;
        let my_addr = self.my_addr.clone();
        let pieces_detail = self.pieces_detail.clone();

        task::spawn(async move {
            let mut peer = match Peer::new(addr, pieces_detail, my_addr).await {
                Ok(peer) => peer,
                Err(e) => {
                    println!("PEER ERROR {:?}", e);
                    return;
                }
            };
            peer.start().await;
        });
    }

    async fn process_cmds(&mut self) {
        use TorrentNotification::*;

        while let Some(msg) = self.receiver.recv().await {
            match msg {
                UpdateBitfield { id, update } => {
                    if self.find_pieces_for_peer(id, &update).await {
                        let peer = self.peers.get(&id).unwrap();
                        peer.addr.send(PeerCommand::TasksAvailables).await;
                    }

                    if let Some(peer) = self.peers.get_mut(&id) {
                        peer.bitfield.update(update);
                    };
                }
                RemovePeer { id, queue } => {
                    self.peers.remove(&id);

                    for piece in self.pieces.iter_mut().filter_map(Option::as_mut) {
                        piece.workers.retain(|p| !Arc::ptr_eq(&p, &queue) );
                    }
                }
                AddPeer { id, queue, addr, socket } => {
                    self.peers.insert(id, PeerState {
                        bitfield: BitField::new(self.pieces_detail.num_pieces),
                        queue_tasks: queue,
                        addr,
                        socket,
                    });
                }
                AddPiece (piece_block) => {
                    let index = piece_block.piece_index;
                    if let Some(sum2) = self.pieces_detail.sha1_pieces.get(index).map(Arc::clone) {

                        let piece_buffer = Arc::new(piece_block);
                        let addr = self.my_addr.clone();

                        let id = self.pending_pieces.insert(Arc::clone(&piece_buffer));

                        self.sha1_workers.send(Sha1Task::CheckSum { piece_buffer, sum2, id, addr });
                    }
                }
                ResultChecksum { id, valid } => {
                    if self.pending_pieces.contains(id) {
                        let _piece = self.pending_pieces.remove(id);
                    };
                    //println!("PIECE CHECKED FROM THE POOL: {}", valid);
                }
                PeerDiscovered { addrs } => {
                    for addr in &addrs {
                        let mut peers = self.peers.values();
                        if peers.position(|p| &p.socket == addr).is_none() {
                            self.connect_to_peers(addr);
                        }
                    }
                }
            }
        }
    }

    async fn find_pieces_for_peer(&mut self, peer: PeerId, update: &BitFieldUpdate) -> bool {
        let mut pieces = &mut self.pieces;
        let nblock_piece = self.pieces_detail.nblocks_piece;
        let block_size = self.pieces_detail.block_size;

        let queue_peer = self.peers.get_mut(&peer).map(|p| &mut p.queue_tasks).unwrap();
        let mut queue = queue_peer.write().await;

        let mut found = false;

        match update {
            BitFieldUpdate::BitField(bitfield) => {
                let pieces = pieces.iter_mut()
                                   .enumerate()
                                   .filter(|(index, p)| p.is_none() && bitfield.get_bit(*index))
                                   .take(20);

                for (piece, value) in pieces {
                    for i in 0..nblock_piece {
                        queue.push_back(PieceToDownload::new(piece, i * block_size, block_size));
                    }
                    //println!("[{:?}] PUSHING PIECE={}", peer.id, piece);
                    value.replace(PieceInfo::new(queue_peer.clone()));
                    if !found {
                        found = true;
                    }
                }
            }
            BitFieldUpdate::Piece(piece) => {
                let piece = *piece;

                if piece >= pieces.len() {
                    return false;
                }

                if pieces.get(piece).unwrap().is_none() {
                    for i in 0..nblock_piece {
                        queue.push_back(PieceToDownload::new(piece, i * block_size, block_size));
                    }
                    //println!("[{:?}] _PUSHING PIECE={}", peer.id, piece);
                    pieces.get_mut(piece).unwrap().replace(PieceInfo::new(queue_peer.clone()));
                    found = true;
                }

            }
        }

        found
    }
}
