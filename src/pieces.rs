use crate::{
    metadata::Torrent,
    peer::message::MessagePeer,
    piece_picker::{BlockIndex, PieceIndex},
};

use std::{fmt::Debug, sync::Arc};

#[derive(Clone)]
pub struct Pieces {
    /// Info hash
    pub info_hash: Arc<[u8]>,
    /// Number of pieces
    pub num_pieces: usize,
    /// SHA1 of each piece
    pub sha1_pieces: Arc<[Arc<[u8; 20]>]>,
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

impl Debug for Pieces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pieces")
            .field("info_hash", &self.info_hash)
            .field("num_pieces", &self.num_pieces)
            .field("sha1_pieces_length", &self.sha1_pieces.len())
            // .field("sha1_pieces", &self.sha1_pieces)
            .field("block_size", &self.block_size)
            .field("last_block_size", &self.last_block_size)
            .field("nblocks_piece", &self.nblocks_piece)
            .field("nblocks_last_piece", &self.nblocks_last_piece)
            .field("piece_length", &self.piece_length)
            .field("last_piece_length", &self.last_piece_length)
            .field("files_size", &self.files_size)
            .finish()
    }
}

impl From<&Torrent> for Pieces {
    fn from(torrent: &Torrent) -> Pieces {
        let sha1_pieces = torrent.sha_pieces();

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
            panic!("Invalid hashes {} {}", sha1_pieces.len(), num_pieces);
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
        let block_index: u32 = block_index.into();
        let piece_length = self.piece_size_of(piece_index);

        assert!(block_index <= piece_length);

        let next_block = (block_index + self.block_size).min(piece_length);

        next_block - block_index
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

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum TaskDownload {
    /// 1 piece
    Piece { piece_index: PieceIndex },
    /// A range of pieces, excluding 'end'
    /// We can't use std::ops::Range because it's not Copy
    PiecesRange { start: PieceIndex, end: PieceIndex },
    /// 1 specific block, excluding 'end'
    /// We can't use std::ops::Range because it's not Copy
    BlockRange {
        piece_index: PieceIndex,
        start: BlockIndex,
        end: BlockIndex,
    },
}

impl Debug for TaskDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskDownload::Piece { piece_index } => {
                write!(f, "TaskDownload::Piece {{ {:?} }}", piece_index)
            }
            TaskDownload::PiecesRange { start, end } => {
                write!(f, "TaskDownload::PiecesRange {{ {:?} {:?} }}", start, end)
            }
            TaskDownload::BlockRange {
                piece_index,
                start,
                end,
            } => {
                write!(
                    f,
                    "TaskDownload::BlockRange {{ {:?} {:?} {:?} }}",
                    piece_index, start, end
                )
            }
        }
    }
}

pub struct IterTaskDownload {
    task: TaskDownload,
    current_piece: PieceIndex,
    current_block: BlockIndex,
    pieces_info: Arc<Pieces>,
}

impl TaskDownload {
    pub fn iter_by_block(self, pieces_info: &Arc<Pieces>) -> IterTaskDownload {
        let (piece, block) = match self {
            TaskDownload::Piece { piece_index } => (piece_index, 0.into()),
            TaskDownload::PiecesRange { start, end: _ } => (start, 0.into()),
            TaskDownload::BlockRange {
                piece_index,
                start,
                end: _,
            } => (piece_index, start),
        };

        IterTaskDownload {
            task: self,
            current_piece: piece,
            current_block: block,
            pieces_info: Arc::clone(pieces_info),
        }
    }
}

impl IterTaskDownload {
    pub(crate) fn is_empty(&self) -> bool {
        match self.task {
            TaskDownload::Piece { piece_index } => {
                let current_block: u32 = self.current_block.into();
                self.pieces_info.piece_size_of(piece_index) == current_block
            }
            TaskDownload::PiecesRange { end, .. } => {
                let piece = self.current_piece;
                let current_block: u32 = self.current_block.into();

                piece.next_piece() == end && self.pieces_info.piece_size_of(piece) == current_block
            }
            TaskDownload::BlockRange { end, .. } => end == self.current_block,
        }
    }
}

impl Iterator for IterTaskDownload {
    type Item = BlockToDownload;

    fn next(&mut self) -> Option<Self::Item> {
        match self.task {
            TaskDownload::Piece { piece_index: piece } => {
                let start = self.current_block;
                let length = self.pieces_info.block_length_of(piece, start);
                self.current_block = (u32::from(start) + length).into();

                if length == 0 {
                    None
                } else {
                    Some(BlockToDownload {
                        piece,
                        start,
                        length,
                    })
                }
            }
            TaskDownload::PiecesRange { start: _, end } => loop {
                let mut piece = self.current_piece;
                let start = self.current_block;
                let length = self.pieces_info.block_length_of(piece, start);
                self.current_block = (u32::from(start) + length).into();

                if length != 0 {
                    return Some(BlockToDownload {
                        piece,
                        start,
                        length,
                    });
                }

                piece = piece.next_piece();

                if piece == end {
                    return None;
                }

                self.current_piece = piece;
                self.current_block = 0.into();
            },
            TaskDownload::BlockRange {
                piece_index,
                start: _,
                end,
            } => {
                let end: u32 = end.into();

                let start = self.current_block;
                let length = self.pieces_info.block_length_of(piece_index, start);
                let end = (u32::from(start) + length).min(end);

                self.current_block = end.into();

                if start == end.into() {
                    None
                } else {
                    Some(BlockToDownload {
                        piece: piece_index,
                        start,
                        length: end - u32::from(start),
                    })
                }
            }
        }
    }
}

#[derive(Eq, PartialEq, Hash, Clone)]
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

impl<'a> From<BlockToDownload> for MessagePeer<'a> {
    fn from(block: BlockToDownload) -> Self {
        MessagePeer::Request {
            piece: block.piece,
            block: block.start,
            length: block.length,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{BlockToDownload, Pieces, TaskDownload};

    #[test]
    fn iter_task() {
        let pieces_info = Arc::new(Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new([]),
            block_size: 100,
            last_block_size: 50,
            nblocks_piece: 13,
            nblocks_last_piece: 8,
            piece_length: 1250,
            last_piece_length: 788,
            files_size: 0,
        });

        let task = TaskDownload::Piece {
            piece_index: 0.into(),
        };
        let iter = task.iter_by_block(&pieces_info);
        assert_eq!(
            iter.collect::<Vec<_>>(),
            &[
                BlockToDownload {
                    piece: 0.into(),
                    start: 0.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 100.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 200.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 300.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 400.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 500.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 600.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 700.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 800.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 900.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 1000.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 1100.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 1200.into(),
                    length: 50
                }
            ]
        );

        let task = TaskDownload::Piece {
            piece_index: 8.into(),
        };
        let iter = task.iter_by_block(&pieces_info);
        assert_eq!(
            iter.collect::<Vec<_>>(),
            &[
                BlockToDownload {
                    piece: 8.into(),
                    start: 0.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 100.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 200.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 300.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 400.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 500.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 600.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 700.into(),
                    length: 88
                },
            ]
        );

        let task = TaskDownload::BlockRange {
            piece_index: 0.into(),
            start: 101.into(),
            end: 653.into(),
        };
        let iter = task.iter_by_block(&pieces_info);
        assert_eq!(
            iter.collect::<Vec<_>>(),
            &[
                BlockToDownload {
                    piece: 0.into(),
                    start: 101.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 201.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 301.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 401.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 501.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 0.into(),
                    start: 601.into(),
                    length: 52
                },
            ]
        );

        let task = TaskDownload::BlockRange {
            piece_index: 8.into(),
            start: 567.into(),
            end: 788.into(),
        };
        let iter = task.iter_by_block(&pieces_info);
        assert_eq!(
            iter.collect::<Vec<_>>(),
            &[
                BlockToDownload {
                    piece: 8.into(),
                    start: 567.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 667.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 767.into(),
                    length: 21
                },
            ]
        );

        let task = TaskDownload::PiecesRange {
            start: 7.into(),
            end: 9.into(),
        };
        let iter = task.iter_by_block(&pieces_info);
        assert_eq!(
            iter.collect::<Vec<_>>(),
            &[
                BlockToDownload {
                    piece: 7.into(),
                    start: 0.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 100.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 200.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 300.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 400.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 500.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 600.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 700.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 800.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 900.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 1000.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 1100.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 7.into(),
                    start: 1200.into(),
                    length: 50
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 0.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 100.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 200.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 300.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 400.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 500.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 600.into(),
                    length: 100
                },
                BlockToDownload {
                    piece: 8.into(),
                    start: 700.into(),
                    length: 88
                },
            ]
        );
    }

    #[test]
    fn block_length() {
        let pieces_info = Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new([]),
            block_size: 100,
            last_block_size: 50,
            nblocks_piece: 13,
            nblocks_last_piece: 8,
            piece_length: 1250,
            last_piece_length: 788,
            files_size: 0,
        };

        assert_eq!(pieces_info.block_length_of(0.into(), 0.into()), 100);
        assert_eq!(pieces_info.block_length_of(0.into(), 1100.into()), 100);
        assert_eq!(pieces_info.block_length_of(0.into(), 1200.into()), 50);

        assert_eq!(pieces_info.block_length_of(8.into(), 0.into()), 100);
        assert_eq!(pieces_info.block_length_of(8.into(), 600.into()), 100);
        assert_eq!(pieces_info.block_length_of(8.into(), 700.into()), 88);
    }

    #[test]
    fn block_length_last_piece_equal() {
        let pieces_info = Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new([]),
            block_size: 100,
            last_block_size: 100,
            nblocks_piece: 13,
            nblocks_last_piece: 8,
            piece_length: 1300,
            last_piece_length: 800,
            files_size: 0,
        };

        assert_eq!(pieces_info.block_length_of(0.into(), 0.into()), 100);
        assert_eq!(pieces_info.block_length_of(0.into(), 1100.into()), 100);
        assert_eq!(pieces_info.block_length_of(0.into(), 1200.into()), 100);

        assert_eq!(pieces_info.block_length_of(8.into(), 0.into()), 100);
        assert_eq!(pieces_info.block_length_of(8.into(), 600.into()), 100);
        assert_eq!(pieces_info.block_length_of(8.into(), 700.into()), 100);
    }

    #[test]
    #[should_panic]
    fn block_length_wrong_block_index() {
        let pieces_info = Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new([]),
            block_size: 100,
            last_block_size: 100,
            nblocks_piece: 13,
            nblocks_last_piece: 8,
            piece_length: 1300,
            last_piece_length: 800,
            files_size: 0,
        };

        pieces_info.block_length_of(8.into(), 801.into());
    }

    #[test]
    #[should_panic]
    fn block_length_panic() {
        let pieces_info = Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new([]),
            block_size: 100,
            last_block_size: 50,
            nblocks_piece: 13,
            nblocks_last_piece: 8,
            piece_length: 1250,
            last_piece_length: 788,
            files_size: 0,
        };

        // Invalid index
        assert_eq!(pieces_info.block_length_of(8.into(), 800.into()), 88);
    }

    #[test]
    fn piece_length() {
        let pieces_info = Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new([]),
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
            sha1_pieces: Arc::new([]),
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

    fn task_download_size() {
        assert_eq!(std::mem::size_of::<TaskDownload>(), 16)
    }
}
