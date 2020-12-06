use crate::{
    metadata::Torrent,
    piece_picker::{BlockIndex, PieceIndex},
};

use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Pieces {
    /// Info hash
    pub info_hash: Arc<[u8]>,
    /// Number of pieces
    pub num_pieces: usize,
    /// SHA1 of each piece
    pub sha1_pieces: Arc<Vec<Arc<[u8; 20]>>>,
    /// Pieces other peers have
    /// peers_pieces[0] is the number of peers having the piece 0
    //    peers_pieces: Vec<u8>,
    /// Size of a block
    pub block_size: u32,
    /// Number of block in 1 piece
    pub nblocks_piece: usize,
    /// Number of block in the last piece
    pub nblocks_last_piece: usize,
    /// Piece length
    pub piece_length: usize,
    /// Last piece length
    pub last_piece_length: usize,
    /// Total files size
    pub files_size: usize,
}

impl From<&Torrent> for Pieces {
    fn from(torrent: &Torrent) -> Pieces {
        let sha1_pieces = Arc::new(torrent.sha_pieces());

        let files_size = torrent.files_total_size();
        let piece_length = torrent.meta.info.piece_length as usize;

        let last_piece_length = files_size % piece_length;
        let last_piece_length = if last_piece_length == 0 {
            piece_length
        } else {
            last_piece_length
        };

        if piece_length == 0 {
            panic!("Invalid piece length");
        }

        let num_pieces = (files_size + piece_length - 1) / piece_length;

        if sha1_pieces.len() != num_pieces {
            panic!("Invalid hashes");
        }

        let block_size = std::cmp::min(piece_length, 0x4000);
        let nblocks_piece = (piece_length + block_size - 1) / block_size;
        let nblocks_last_piece = ((files_size % piece_length) + block_size - 1) / block_size;

        Pieces {
            info_hash: Arc::clone(&torrent.info_hash),
            num_pieces,
            sha1_pieces,
            block_size: block_size as u32,
            nblocks_piece,
            nblocks_last_piece,
            piece_length,
            last_piece_length,
            files_size,
        }
    }
}

//use bit_field::BitArray;

// TODO:
// See https://doc.rust-lang.org/1.29.0/core/arch/x86_64/fn._popcnt64.html
// https://github.com/rust-lang/packed_simd
// https://stackoverflow.com/questions/42938907/is-it-possible-to-use-simd-instructions-in-rust

impl Pieces {
    // fn update(&mut self, update: &BitFieldUpdate)  {
    //     match update {
    //         BitFieldUpdate::BitField(bitfield) => {
    //             for (i, piece) in self.peers_pieces.iter_mut().enumerate() {
    //                 if bitfield.get_bit(i) {
    //                     *piece = piece.saturating_add(1);
    //                 }
    //             }
    //         }
    //         BitFieldUpdate::Piece(piece) => {
    //             // TODO: Handle inexistant piece
    //             if let Some(piece) = self.peers_pieces.get_mut(*piece) {
    //                 *piece = piece.saturating_add(1);
    //             }
    //         }
    //     }
    // }

    pub fn piece_size_of(&self, piece_index: PieceIndex) -> u32 {
        let piece_index: usize = piece_index.into();

        if piece_index == self.num_pieces - 1 {
            self.last_piece_length as u32
        } else {
            self.piece_length as u32
        }
    }
}

#[derive(Clone, Debug)]
pub struct BlockToDownload {
    pub piece: PieceIndex,
    pub start: BlockIndex,
    pub length: u32,
}

impl BlockToDownload {
    pub fn new(piece: PieceIndex, start: BlockIndex, length: u32) -> BlockToDownload {
        BlockToDownload {
            piece,
            start,
            length,
        }
    }
}
