use crate::{
    actors::peer::MessagePeer,
    metadata::Torrent,
    piece_picker::{BlockIndex, PieceIndex},
};

use std::{fmt::Debug, ops::Range, sync::Arc};

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
    /// Size of the last block in a piece
    pub last_block_size: u32,
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

        let mut last_piece_length = files_size % piece_length;
        if last_piece_length == 0 {
            last_piece_length = piece_length;
        }

        if piece_length == 0 {
            panic!("Invalid piece length");
        }

        let num_pieces = (files_size + piece_length - 1) / piece_length;

        if sha1_pieces.len() != num_pieces {
            panic!("Invalid hashes");
        }

        let block_size = piece_length.min(0x4000);
        let nblocks_piece = (piece_length + block_size - 1) / block_size;
        let nblocks_last_piece = ((files_size % piece_length) + block_size - 1) / block_size;

        let block_size = block_size as u32;
        let mut last_block_size = piece_length as u32 % block_size;
        if last_block_size == 0 {
            last_block_size = block_size;
        }

        Pieces {
            info_hash: Arc::clone(&torrent.info_hash),
            num_pieces,
            sha1_pieces,
            block_size,
            last_block_size,
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
    pub fn block_length_of(&self, piece_index: PieceIndex, block_index: BlockIndex) -> u32 {
        let block_index: usize = block_index.into();

        assert!(block_index < self.nblocks_piece);

        if self.is_last_piece(piece_index) {
            assert!(block_index < self.nblocks_last_piece);

            if block_index == self.nblocks_last_piece - 1 {
                let block_size = self.last_piece_length as u32 % self.block_size;
                if block_size == 0 {
                    self.block_size
                } else {
                    block_size
                }
            } else {
                self.block_size
            }
        } else {
            let block_size = self.block_size;

            if block_index == self.nblocks_piece - 1 {
                self.last_block_size
            } else {
                block_size
            }
        }
    }

    pub fn nblock_in_piece(&self, piece_index: PieceIndex) -> usize {
        if self.is_last_piece(piece_index) {
            self.nblocks_last_piece
        } else {
            self.nblocks_piece
        }
    }

    fn is_last_piece(&self, piece_index: PieceIndex) -> bool {
        let piece_index: usize = piece_index.into();
        assert!(piece_index < self.num_pieces);

        piece_index == self.num_pieces - 1
    }

    pub fn piece_size_of(&self, piece_index: PieceIndex) -> u32 {
        if self.is_last_piece(piece_index) {
            self.last_piece_length as u32
        } else {
            self.piece_length as u32
        }
    }
}

#[derive(Debug)]
pub enum TaskDownload {
    /// 1 piece
    Piece(PieceIndex),
    /// A range of pieces
    PiecesRange(Range<PieceIndex>),
    // Block(BlockToDownload),
    /// 1 specific block
    BlockRange {
        piece_index: PieceIndex,
        range: Range<u32>,
    },
}

pub struct BlockToDownload {
    pub piece: PieceIndex,
    pub start: BlockIndex,
    pub length: u32,
}

impl Debug for BlockToDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BlockToDownload {{ {:?}, {:?} length={:?} }}",
            self.piece, self.start, self.length
        )
    }
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

impl<'a> Into<MessagePeer<'a>> for BlockToDownload {
    fn into(self) -> MessagePeer<'a> {
        MessagePeer::Request {
            index: self.piece,
            begin: self.start,
            length: self.length,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::Pieces;

    #[test]
    fn block_length() {
        let pieces_info = Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new(Vec::new()),
            block_size: 100,
            last_block_size: 50,
            nblocks_piece: 13,
            nblocks_last_piece: 8,
            piece_length: 1250,
            last_piece_length: 788,
            files_size: 0,
        };

        assert_eq!(pieces_info.block_length_of(0.into(), 0.into()), 100);
        assert_eq!(pieces_info.block_length_of(0.into(), 11.into()), 100);
        assert_eq!(pieces_info.block_length_of(0.into(), 12.into()), 50);

        assert_eq!(pieces_info.block_length_of(8.into(), 0.into()), 100);
        assert_eq!(pieces_info.block_length_of(8.into(), 6.into()), 100);
        assert_eq!(pieces_info.block_length_of(8.into(), 7.into()), 88);
    }

    #[test]
    fn block_length_last_piece_equal() {
        let pieces_info = Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new(Vec::new()),
            block_size: 100,
            last_block_size: 100,
            nblocks_piece: 13,
            nblocks_last_piece: 8,
            piece_length: 1300,
            last_piece_length: 800,
            files_size: 0,
        };

        assert_eq!(pieces_info.block_length_of(0.into(), 0.into()), 100);
        assert_eq!(pieces_info.block_length_of(0.into(), 11.into()), 100);
        assert_eq!(pieces_info.block_length_of(0.into(), 12.into()), 100);

        assert_eq!(pieces_info.block_length_of(8.into(), 0.into()), 100);
        assert_eq!(pieces_info.block_length_of(8.into(), 6.into()), 100);
        assert_eq!(pieces_info.block_length_of(8.into(), 7.into()), 100);
    }

    #[test]
    #[should_panic]
    fn block_length_panic() {
        let pieces_info = Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new(Vec::new()),
            block_size: 100,
            last_block_size: 50,
            nblocks_piece: 13,
            nblocks_last_piece: 8,
            piece_length: 1250,
            last_piece_length: 788,
            files_size: 0,
        };

        // Invalid index
        assert_eq!(pieces_info.block_length_of(8.into(), 8.into()), 88);
    }

    #[test]
    fn piece_length() {
        let pieces_info = Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new(Vec::new()),
            block_size: 100,
            last_block_size: 50,
            nblocks_piece: 13,
            nblocks_last_piece: 7,
            piece_length: 1250,
            last_piece_length: 788,
            files_size: 0,
        };

        for index in 0..8 {
            assert_eq!(pieces_info.piece_size_of(index.into()), 1250);
        }

        assert_eq!(pieces_info.piece_size_of(8.into()), 788);
    }

    #[test]
    #[should_panic]
    fn piece_length_panic() {
        let pieces_info = Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new(Vec::new()),
            block_size: 100,
            last_block_size: 50,
            nblocks_piece: 13,
            nblocks_last_piece: 7,
            piece_length: 1250,
            last_piece_length: 788,
            files_size: 0,
        };

        // Invalid index
        assert_eq!(pieces_info.piece_size_of(9.into()), 788);
    }
}
