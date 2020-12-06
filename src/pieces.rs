use crate::{actors::peer::PeerTask, metadata::Torrent, piece_picker::PieceIndex};
use smallvec::{smallvec, SmallVec};

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
    pub block_size: usize,
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
            block_size,
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

    pub fn piece_size(&self, piece_index: usize) -> usize {
        if piece_index == self.num_pieces - 1 {
            self.last_piece_length
        } else {
            self.piece_length
        }
    }

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
pub struct PieceToDownload {
    pub piece: u32,
    pub start: u32,
    pub size: u32,
}

impl PieceToDownload {
    pub fn new(piece: u32, start: usize, size: usize) -> PieceToDownload {
        PieceToDownload {
            piece,
            start: start as u32,
            size: size as u32,
        }
    }
}

#[derive(Debug)]
pub struct PieceInfo {
    pub bytes_downloaded: usize,
    pub workers: SmallVec<[PeerTask; 4]>,
}

impl PieceInfo {
    pub fn new(queue: PeerTask) -> PieceInfo {
        PieceInfo {
            bytes_downloaded: 0,
            workers: smallvec![queue],
        }
    }

    // fn new(peer: &Peer) -> PieceInfo {
    //     PieceInfo {
    //         bytes_downloaded: 0,
    //         workers: smallvec![peer.tasks.clone()]
    //     }
    // }
}

pub struct PieceBuffer {
    pub buf: Box<[u8]>,
    pub piece_index: usize,
    pub bytes_added: usize,
}

impl PieceBuffer {
    /// piece_size might be different than piece_length if it's the last piece
    fn new(piece_index: usize, piece_size: usize) -> PieceBuffer {
        let mut buf = Vec::with_capacity(piece_size);
        unsafe { buf.set_len(piece_size) };
        let buf = buf.into_boxed_slice();

        // println!("ADDING PIECE_BUFFER {} SIZE={}", piece_index, piece_size);

        PieceBuffer {
            buf,
            piece_index,
            bytes_added: 0,
        }
    }

    pub fn new_with_block(
        piece_index: u32,
        piece_size: usize,
        begin: u32,
        block: &[u8],
    ) -> PieceBuffer {
        let mut piece_buffer = PieceBuffer::new(piece_index as usize, piece_size);
        piece_buffer.add_block(begin, block);
        piece_buffer
    }

    /// begin: offset of the block in the piece
    pub fn add_block(&mut self, begin: u32, block: &[u8]) {
        let block_len = block.len();

        let begin = begin as usize;
        let buffer_len = self.buf.len();
        if let Some(buffer) = self.buf.get_mut(begin..begin + block_len) {
            buffer.copy_from_slice(block);
            self.bytes_added += block_len;
        } else {
            panic!(
                "ERROR on addblock BEGIN={} BLOCK_LEN={} BUF_LEN={}",
                begin, block_len, buffer_len
            );
        }
    }

    fn added(&self) -> usize {
        self.bytes_added
    }

    /// The piece is considered completed once added block size match with
    /// the piece size. There is no check where the blocks have been added
    pub fn is_completed(&self) -> bool {
        self.buf.len() == self.bytes_added
    }
}
