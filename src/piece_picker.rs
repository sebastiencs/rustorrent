use std::sync::Arc;

use kv_log_macro::{error, info};
use smallvec::SmallVec;

use crate::{
    actors::peer::PeerId,
    bitfield::{BitField, BitFieldUpdate},
    pieces::Pieces,
};

#[derive(Eq, PartialEq, Hash, Copy, Clone, Debug)]
pub struct PieceIndex(u32);

/// Offset of the block in the piece
#[derive(Eq, PartialEq, Hash, Copy, Clone, Debug)]
pub struct BlockIndex(u32);

impl From<PieceIndex> for usize {
    fn from(piece_index: PieceIndex) -> Self {
        piece_index.0 as usize
    }
}

impl From<PieceIndex> for u32 {
    fn from(piece_index: PieceIndex) -> Self {
        piece_index.0
    }
}

impl From<BlockIndex> for u32 {
    fn from(block_index: BlockIndex) -> Self {
        block_index.0
    }
}

impl From<u32> for BlockIndex {
    fn from(block_index: u32) -> Self {
        BlockIndex(block_index)
    }
}

impl From<u32> for PieceIndex {
    fn from(index: u32) -> Self {
        PieceIndex(index)
    }
}

/// Number of peers per piece
//#[derive(Eq)]
struct PeersPerPiece {
    /// Number of peers having this piece
    npeer: u32,
    /// Piece index
    index: PieceIndex,
}

impl PartialEq for PeersPerPiece {
    fn eq(&self, other: &Self) -> bool {
        self.npeer == other.npeer
    }
}

impl Eq for PeersPerPiece {}

impl PartialOrd for PeersPerPiece {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.npeer.cmp(&other.npeer))
    }
}

impl Ord for PeersPerPiece {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.npeer.cmp(&other.npeer)
    }
}

impl PeersPerPiece {
    fn new(index: PieceIndex) -> PeersPerPiece {
        PeersPerPiece { npeer: 0, index }
    }
}

struct PieceState {
    downloaded: bool,
    workers: SmallVec<[PeerId; 4]>,
}

impl PieceState {
    fn new() -> PieceState {
        PieceState {
            downloaded: false,
            workers: SmallVec::new(),
        }
    }
}

pub struct PiecePicker {
    pieces_infos: Arc<Pieces>,

    /// The first `start_at` pieces are already downloaded
    start_at: usize,
    ///
    sorted_index: Box<[PeersPerPiece]>,

    /// states[9] is the state of the piece at index 9
    states: Box<[PieceState]>,
}

impl PiecePicker {
    pub fn new(pieces_info: &Arc<Pieces>) -> PiecePicker {
        let num_pieces = pieces_info.num_pieces;

        let mut sorted_index = Vec::with_capacity(num_pieces);
        let mut states = Vec::with_capacity(num_pieces);

        for index in 0..num_pieces {
            let index = PieceIndex(index as u32);

            sorted_index.push(PeersPerPiece::new(index));
            states.push(PieceState::new());
        }

        PiecePicker {
            start_at: 0,
            states: states.into_boxed_slice(),
            sorted_index: sorted_index.into_boxed_slice(),
            pieces_infos: Arc::clone(pieces_info),
        }
    }

    pub fn pick_piece(&mut self, peer_id: PeerId, bitfield: &BitField) -> Option<PieceIndex> {
        if let Some(peers_per_piece) =
            self.sorted_index[self.start_at..]
                .iter()
                .find(|peers_per_piece| {
                    self.states[usize::from(peers_per_piece.index)]
                        .workers
                        .is_empty()
                        && bitfield.get_bit(peers_per_piece.index)
                })
        {
            info!("[{}] Found piece {:?}", peer_id, peers_per_piece.index);
            self.states[usize::from(peers_per_piece.index)]
                .workers
                .push(peer_id);
            return Some(peers_per_piece.index);
        }

        if let Some(peers_per_piece) = self.sorted_index[self.start_at..]
            .iter()
            .find(|peers_per_piece| bitfield.get_bit(peers_per_piece.index))
        {
            info!(
                "[{}] All pieces taken. Found piece {:?}",
                peer_id, peers_per_piece.index
            );
            self.states[usize::from(peers_per_piece.index)]
                .workers
                .push(peer_id);
            return Some(peers_per_piece.index);
        }

        error!("[{}] No piece found", peer_id);
        None
    }

    pub fn remove_peer(&mut self, peer_id: PeerId) {
        for state in &mut *self.states {
            if let Some(index) = state.workers.iter().position(|id| *id == peer_id) {
                state.workers.remove(index);
            }
        }
    }

    pub fn update(&mut self, update: &BitFieldUpdate) {
        match update {
            BitFieldUpdate::BitField(bitfield) => {
                for peers_per_piece in &mut *self.sorted_index {
                    if bitfield.get_bit(peers_per_piece.index) {
                        peers_per_piece.npeer += 1;
                    }
                }
            }
            BitFieldUpdate::Piece(piece_index) => {
                let peers_per_piece = self
                    .sorted_index
                    .iter_mut()
                    .find(|ppp| ppp.index == *piece_index);

                if let Some(peers_per_piece) = peers_per_piece {
                    peers_per_piece.npeer += 1;
                };
            }
        }

        self.sort_indexed();
    }

    fn sort_indexed(&mut self) {
        self.sorted_index.sort_unstable();
        self.start_at = self
            .sorted_index
            .iter()
            .position(|ppp| !self.states[usize::from(ppp.index)].downloaded)
            .unwrap_or_else(|| self.sorted_index.len());
    }
}
