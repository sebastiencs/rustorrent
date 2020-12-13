use std::{fmt::Debug, sync::Arc};

use fastrand::Rng;
//use kv_log_macro::info;

use crate::{
    actors::peer::PeerId,
    bitfield::{BitField, BitFieldUpdate},
    piece_collector::PieceCollector,
    pieces::{Pieces, TaskDownload},
    utils::Set,
};

#[derive(Eq, PartialEq, Hash, Copy, Clone, Debug, Ord, PartialOrd)]
pub struct PieceIndex(u32);

/// Offset of the block in the piece
#[derive(Eq, PartialEq, Hash, Copy, Clone, Debug, Ord, PartialOrd)]
pub struct BlockIndex(u32);

impl From<PieceIndex> for usize {
    fn from(piece_index: PieceIndex) -> Self {
        piece_index.0 as usize
    }
}

impl From<BlockIndex> for usize {
    fn from(block_index: BlockIndex) -> Self {
        block_index.0 as usize
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

impl PieceIndex {
    fn next_piece(self) -> PieceIndex {
        (self.0 + 1).into()
    }
}

/// Number of peers per piece
struct PeersPerPiece {
    /// Number of peers having this piece
    npeers: u32,
    /// Piece index
    index: PieceIndex,
}

impl Debug for PeersPerPiece {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PeersPerPiece {{ npeers: {}, {:?} }}",
            self.npeers, self.index
        )
    }
}

impl PartialEq for PeersPerPiece {
    fn eq(&self, other: &Self) -> bool {
        self.npeers == other.npeers
    }
}

impl Eq for PeersPerPiece {}

impl PartialOrd for PeersPerPiece {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.npeers.cmp(&other.npeers))
    }
}

impl Ord for PeersPerPiece {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.npeers.cmp(&other.npeers)
    }
}

impl PeersPerPiece {
    fn new(index: PieceIndex) -> PeersPerPiece {
        PeersPerPiece { npeers: 0, index }
    }
}

struct PieceState {
    downloaded: bool,
    workers: Set<PeerId>,
    // workers: SmallVec<[PeerId; 4]>,
}

impl Debug for PieceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PieceState {{ downloaded: {}, {:?} }}",
            self.downloaded, self.workers
        )
    }
}

impl PieceState {
    fn new() -> PieceState {
        PieceState {
            downloaded: false,
            workers: Set::default(),
            //workers: SmallVec::new(),
        }
    }
}

#[derive(Debug)]
pub struct PiecePicker {
    pieces_infos: Arc<Pieces>,

    /// The first `start_at` pieces are already downloaded
    start_at: usize,
    ///
    sorted_index: Box<[PeersPerPiece]>,

    /// states[9] is the state of the piece at index 9
    states: Box<[PieceState]>,

    to_download: Vec<TaskDownload>,

    haves: Vec<PieceIndex>,
    rng: Rng,
}

enum Picked {
    Full(PieceIndex),
    Partial(PieceIndex),
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
            to_download: Vec::with_capacity(256),
            rng: Rng::new(),
            haves: Vec::with_capacity(256),
        }
    }

    pub fn set_as_downloaded(&mut self, piece: PieceIndex) {
        let index: usize = piece.into();
        self.states[index].downloaded = true;
        self.sort_indexed();
    }

    fn add_piece_to_download(&mut self, piece_index: PieceIndex) {
        // We're only interested with the last added piece, because
        // previous pieces could have a higher priority
        if let Some(to_download) = self.to_download.last_mut() {
            match to_download {
                TaskDownload::Piece { piece_index: p } if p.next_piece() == piece_index => {
                    *to_download = TaskDownload::PiecesRange {
                        start: *p,
                        end: piece_index.next_piece(),
                    };
                    return;
                }
                TaskDownload::PiecesRange { start: _, end } if *end == piece_index => {
                    *end = piece_index.next_piece();
                    return;
                }
                _ => {}
            }
        };
        self.to_download.push(TaskDownload::Piece { piece_index });
    }

    /// TODO: This might use generator once it's stable
    fn pick_piece_inner(
        &mut self,
        peer_id: PeerId,
        bitfield: &BitField,
        collector: &PieceCollector,
        mut fun: impl FnMut(&mut PiecePicker, Picked) -> bool,
    ) {
        let mut npeers_current = self.sorted_index[self.start_at].npeers;
        let mut start_at = self.start_at;

        // while let Some(peers_per_piece) = iter.next() {
        // for peers_per_piece in &self.sorted_index[self.start_at..] {
        while let Some(peers_per_piece) = self.sorted_index.get(start_at) {
            start_at += 1;

            let state = &self.states[usize::from(peers_per_piece.index)];

            if state.downloaded {
                continue;
            }

            if state.workers.contains(&peer_id) {
                continue;
            }

            let is_empty = state.workers.is_empty();

            let npeers = peers_per_piece.npeers;
            let piece_index = peers_per_piece.index;

            if peers_per_piece.npeers != npeers_current {
                if !self.haves.is_empty() {
                    // Do not use randomness during tests, for assertions in tests
                    #[cfg(not(test))]
                    self.rng.shuffle(&mut self.haves);

                    for index in 0..self.haves.len() {
                        let piece_index = self.haves[index];

                        let cont = if collector.is_empty(piece_index) {
                            fun(self, Picked::Full(piece_index))
                        } else {
                            fun(self, Picked::Partial(piece_index))
                        };

                        if !cont {
                            return;
                        }
                    }

                    self.haves.clear();

                    // break;
                }
                npeers_current = npeers;
            }

            let have = bitfield.get_bit(piece_index);

            if have && is_empty {
                if !fun(self, Picked::Full(piece_index)) {
                    return;
                }
                continue;
                // return Some(Picked::Full(peers_per_piece.index));
            }

            if have {
                println!("HAVE {:?} self.haves={:?}", piece_index, self.haves);
                self.haves.push(piece_index);
            }
        }

        if !self.haves.is_empty() {
            // Do not use randomness during tests, for assertions in tests
            #[cfg(not(test))]
            self.rng.shuffle(&mut self.haves);

            for index in 0..self.haves.len() {
                let piece_index = self.haves[index];

                let cont = if collector.is_empty(piece_index) {
                    fun(self, Picked::Full(piece_index))
                } else {
                    fun(self, Picked::Partial(piece_index))
                };

                if !cont {
                    return;
                }
            }
        }
    }

    pub fn pick_piece(
        &mut self,
        peer_id: PeerId,
        tasks_nbytes: usize,
        bitfield: &BitField,
        collector: &PieceCollector,
    ) -> Option<&[TaskDownload]> {
        let mut tasks_nbytes = tasks_nbytes;

        self.to_download.clear();
        self.haves.clear();

        self.pick_piece_inner(peer_id, bitfield, collector, |picker, piece_index| {
            match piece_index {
                Picked::Full(piece_index) => {
                    picker.states[usize::from(piece_index)]
                        .workers
                        .insert(peer_id);

                    let piece_length = picker.pieces_infos.piece_size_of(piece_index);

                    tasks_nbytes = tasks_nbytes.saturating_sub(piece_length as usize);

                    picker.add_piece_to_download(piece_index);
                }
                Picked::Partial(piece_index) => {
                    let mut found = false;

                    for next_empty in collector.iter_empty_ranges(piece_index) {
                        found = true;

                        tasks_nbytes = tasks_nbytes
                            .saturating_sub((next_empty.end - next_empty.start) as usize);

                        picker.to_download.push(TaskDownload::BlockRange {
                            piece_index,
                            start: next_empty.start.into(),
                            end: next_empty.end.into(),
                        });
                    }

                    if found {
                        picker.states[usize::from(piece_index)]
                            .workers
                            .insert(peer_id);
                    }
                }
            }

            tasks_nbytes > 0
        });

        if self.to_download.is_empty() {
            None
        } else {
            Some(&self.to_download)
        }
    }

    pub fn remove_peer(&mut self, peer_id: PeerId) {
        for state in &mut *self.states {
            state.workers.remove(&peer_id);
            // if let Some(index) = state.workers.iter().position(|id| *id == peer_id) {
            //     state.workers.remove(index);
            // }
        }
    }

    pub fn update(&mut self, update: &BitFieldUpdate) {
        match update {
            BitFieldUpdate::BitField(bitfield) => {
                for peers_per_piece in &mut *self.sorted_index {
                    if bitfield.get_bit(peers_per_piece.index) {
                        peers_per_piece.npeers += 1;
                    }
                }
            }
            BitFieldUpdate::Piece(piece_index) => {
                let peers_per_piece = self
                    .sorted_index
                    .iter_mut()
                    .find(|ppp| ppp.index == *piece_index);

                if let Some(peers_per_piece) = peers_per_piece {
                    peers_per_piece.npeers += 1;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        actors::peer::PeerId,
        bitfield::{BitField, BitFieldUpdate},
        logger,
        piece_collector::{Block, PieceCollector},
        pieces::{BlockToDownload, Pieces, TaskDownload},
    };

    use super::{BlockIndex, PeersPerPiece, PieceIndex, PiecePicker, PieceState};

    fn assert_eq_states(states: &[PieceState], cmp: &[(bool, &[PeerId])]) {
        assert_eq!(states.len(), cmp.len());

        for (s, c) in states.iter().zip(cmp) {
            assert_eq!(s.downloaded, c.0);
            assert_eq!(
                s.workers.len(),
                c.1.len(),
                "length differ {:#?}\n{:#?}",
                states,
                cmp
            );

            for id in c.1 {
                assert!(s.workers.contains(id));
            }
        }
    }

    fn assert_eq_sorted_index(sorted: &[PeersPerPiece], cmp: &[(u32, PieceIndex)]) {
        assert_eq!(sorted.len(), cmp.len());

        for (s, c) in sorted.iter().zip(cmp) {
            assert_eq!(s.npeers, c.0);
            assert_eq!(s.index, c.1);
        }
    }

    #[test]
    fn add_piece_to_download() {
        let pieces_info = Arc::new(Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new(Vec::new()),
            block_size: 100,
            last_block_size: 50,
            nblocks_piece: 13,
            nblocks_last_piece: 7,
            piece_length: 1250,
            last_piece_length: 114688,
            files_size: 0,
        });

        let mut picker = PiecePicker::new(&pieces_info);

        picker.add_piece_to_download(1.into());
        assert_eq!(
            &picker.to_download,
            &[TaskDownload::Piece {
                piece_index: 1.into()
            }]
        );

        picker.add_piece_to_download(2.into());
        assert_eq!(
            &picker.to_download,
            &[TaskDownload::PiecesRange {
                start: 1.into(),
                end: 3.into()
            }]
        );

        picker.add_piece_to_download(3.into());
        assert_eq!(
            &picker.to_download,
            &[TaskDownload::PiecesRange {
                start: 1.into(),
                end: 4.into()
            }]
        );

        picker.add_piece_to_download(5.into());
        assert_eq!(
            &picker.to_download,
            &[
                TaskDownload::PiecesRange {
                    start: 1.into(),
                    end: 4.into()
                },
                TaskDownload::Piece {
                    piece_index: 5.into()
                }
            ]
        );

        picker.add_piece_to_download(0.into());
        assert_eq!(
            &picker.to_download,
            &[
                TaskDownload::PiecesRange {
                    start: 1.into(),
                    end: 4.into()
                },
                TaskDownload::Piece {
                    piece_index: 5.into()
                },
                TaskDownload::Piece {
                    piece_index: 0.into()
                },
            ]
        );
    }

    #[test]
    fn picker() {
        logger::start();

        println!(
            "BlockToDownload {:?}",
            std::mem::size_of::<BlockToDownload>()
        );
        println!("TaskDownload {:?}", std::mem::size_of::<TaskDownload>());

        let pieces_info = Arc::new(Pieces {
            info_hash: Arc::new([]),
            num_pieces: 9,
            sha1_pieces: Arc::new(Vec::new()),
            block_size: 100,
            last_block_size: 50,
            nblocks_piece: 13,
            nblocks_last_piece: 8,
            piece_length: 1250,
            last_piece_length: 791,
            files_size: 0,
        });

        let piece_length = pieces_info.piece_length;

        let mut picker = PiecePicker::new(&pieces_info);
        let mut collector = PieceCollector::new(&pieces_info);

        picker.update(&BitFieldUpdate::Piece(1.into()));
        picker.update(&BitFieldUpdate::Piece(3.into()));
        picker.update(&BitFieldUpdate::Piece(4.into()));
        picker.update(&BitFieldUpdate::Piece(5.into()));
        picker.update(&BitFieldUpdate::Piece(7.into()));
        picker.update(&BitFieldUpdate::Piece(8.into()));
        picker.update(&BitFieldUpdate::Piece(5.into()));
        picker.update(&BitFieldUpdate::Piece(7.into()));

        assert_eq!(
            picker
                .sorted_index
                .iter()
                .map(|a| a.npeers)
                .collect::<Vec<_>>(),
            [0, 0, 0, 1, 1, 1, 1, 2, 2]
        );

        assert_eq_sorted_index(
            &picker.sorted_index,
            &[
                (0, 0.into()),
                (0, 2.into()),
                (0, 6.into()),
                (1, 8.into()),
                (1, 4.into()),
                (1, 3.into()),
                (1, 1.into()),
                (2, 7.into()),
                (2, 5.into()),
            ],
        );

        let bitfield = BitField::from(&[0b11111111, 0b11111111], 9).unwrap();

        let peer1 = PeerId::new(1);
        let to_download = picker.pick_piece(peer1, piece_length * 2, &bitfield, &collector);

        assert_eq!(
            to_download.unwrap(),
            &[
                TaskDownload::Piece {
                    piece_index: 0.into()
                },
                TaskDownload::Piece {
                    piece_index: 2.into()
                },
            ]
        );

        let to_download =
            picker.pick_piece(PeerId::new(2), piece_length + 21, &bitfield, &collector);

        assert_eq!(
            to_download.unwrap(),
            &[
                TaskDownload::Piece {
                    piece_index: 6.into()
                },
                TaskDownload::Piece {
                    piece_index: 0.into()
                },
            ]
        );

        assert_eq_states(
            &picker.states,
            &[
                (false, &[PeerId::new(1), PeerId::new(2)]),
                (false, &[]),
                (false, &[PeerId::new(1)]),
                (false, &[]),
                (false, &[]),
                (false, &[]),
                (false, &[PeerId::new(2)]),
                (false, &[]),
                (false, &[]),
            ],
        );

        collector.add_block(&Block {
            piece_index: 2.into(),
            index: 122.into(),
            block: vec![0; 842].into_boxed_slice(),
        });

        let to_download =
            picker.pick_piece(PeerId::new(3), piece_length * 3, &bitfield, &collector);

        assert_eq!(
            to_download.unwrap(),
            &[
                TaskDownload::Piece {
                    piece_index: PieceIndex(0)
                },
                TaskDownload::BlockRange {
                    piece_index: 2.into(),
                    start: 0.into(),
                    end: 100.into()
                },
                TaskDownload::BlockRange {
                    piece_index: 2.into(),
                    start: 100.into(),
                    end: 122.into()
                },
                TaskDownload::BlockRange {
                    piece_index: 2.into(),
                    start: 964.into(),
                    end: 1064.into()
                },
                TaskDownload::BlockRange {
                    piece_index: 2.into(),
                    start: 1064.into(),
                    end: 1164.into()
                },
                TaskDownload::BlockRange {
                    piece_index: 2.into(),
                    start: 1164.into(),
                    end: 1250.into()
                },
                TaskDownload::Piece {
                    piece_index: PieceIndex(6)
                },
                TaskDownload::Piece {
                    piece_index: PieceIndex(8)
                },
                TaskDownload::Piece {
                    piece_index: PieceIndex(4)
                },
            ]
        );

        assert_eq_states(
            &picker.states,
            &[
                (false, &[PeerId::new(1), PeerId::new(2), PeerId::new(3)]),
                (false, &[]),
                (false, &[PeerId::new(1), PeerId::new(3)]),
                (false, &[]),
                (false, &[PeerId::new(3)]),
                (false, &[]),
                (false, &[PeerId::new(2), PeerId::new(3)]),
                (false, &[]),
                (false, &[PeerId::new(3)]),
            ],
        );

        picker.set_as_downloaded(6.into());
        picker.set_as_downloaded(2.into());

        let to_download =
            picker.pick_piece(PeerId::new(4), piece_length * 2, &bitfield, &collector);

        assert_eq!(
            to_download.unwrap(),
            &[
                TaskDownload::Piece {
                    piece_index: PieceIndex(0)
                },
                TaskDownload::Piece {
                    piece_index: PieceIndex(3)
                },
            ]
        );

        assert_eq_states(
            &picker.states,
            &[
                (
                    false,
                    &[
                        PeerId::new(1),
                        PeerId::new(2),
                        PeerId::new(3),
                        PeerId::new(4),
                    ],
                ),
                (false, &[]),
                (true, &[PeerId::new(1), PeerId::new(3)]),
                (false, &[PeerId::new(4)]),
                (false, &[PeerId::new(3)]),
                (false, &[]),
                (true, &[PeerId::new(2), PeerId::new(3)]),
                (false, &[]),
                (false, &[PeerId::new(3)]),
            ],
        );

        let to_download =
            picker.pick_piece(PeerId::new(4), piece_length * 8, &bitfield, &collector);

        assert_eq!(
            to_download.unwrap(),
            &[
                TaskDownload::Piece {
                    piece_index: PieceIndex(1)
                },
                TaskDownload::Piece {
                    piece_index: PieceIndex(8)
                },
                TaskDownload::Piece {
                    piece_index: PieceIndex(4)
                },
                TaskDownload::Piece {
                    piece_index: PieceIndex(7)
                },
                TaskDownload::Piece {
                    piece_index: PieceIndex(5)
                },
            ]
        );

        assert_eq_states(
            &picker.states,
            &[
                (
                    false,
                    &[
                        PeerId::new(1),
                        PeerId::new(2),
                        PeerId::new(3),
                        PeerId::new(4),
                    ],
                ),
                (false, &[PeerId::new(4)]),
                (true, &[PeerId::new(1), PeerId::new(3)]),
                (false, &[PeerId::new(4)]),
                (false, &[PeerId::new(3), PeerId::new(4)]),
                (false, &[PeerId::new(4)]),
                (true, &[PeerId::new(2), PeerId::new(3)]),
                (false, &[PeerId::new(4)]),
                (false, &[PeerId::new(3), PeerId::new(4)]),
            ],
        );

        let mut bitfield2 = BitField::from(&[0, 0], 9).unwrap();
        bitfield2.set_bit(0usize);
        bitfield2.set_bit(7usize);
        bitfield2.set_bit(8usize);

        let to_download =
            picker.pick_piece(PeerId::new(5), piece_length * 8, &bitfield2, &collector);

        assert_eq!(
            to_download.unwrap(),
            &[
                TaskDownload::Piece {
                    piece_index: PieceIndex(0)
                },
                TaskDownload::Piece {
                    piece_index: PieceIndex(8)
                },
                TaskDownload::Piece {
                    piece_index: PieceIndex(7)
                },
            ]
        );

        assert_eq_states(
            &picker.states,
            &[
                (
                    false,
                    &[
                        PeerId::new(1),
                        PeerId::new(2),
                        PeerId::new(3),
                        PeerId::new(4),
                        PeerId::new(5),
                    ],
                ),
                (false, &[PeerId::new(4)]),
                (true, &[PeerId::new(1), PeerId::new(3)]),
                (false, &[PeerId::new(4)]),
                (false, &[PeerId::new(3), PeerId::new(4)]),
                (false, &[PeerId::new(4)]),
                (true, &[PeerId::new(2), PeerId::new(3)]),
                (false, &[PeerId::new(4), PeerId::new(5)]),
                (false, &[PeerId::new(3), PeerId::new(4), PeerId::new(5)]),
            ],
        );

        picker.remove_peer(PeerId::new(5));

        assert_eq_states(
            &picker.states,
            &[
                (
                    false,
                    &[
                        PeerId::new(1),
                        PeerId::new(2),
                        PeerId::new(3),
                        PeerId::new(4),
                    ],
                ),
                (false, &[PeerId::new(4)]),
                (true, &[PeerId::new(1), PeerId::new(3)]),
                (false, &[PeerId::new(4)]),
                (false, &[PeerId::new(3), PeerId::new(4)]),
                (false, &[PeerId::new(4)]),
                (true, &[PeerId::new(2), PeerId::new(3)]),
                (false, &[PeerId::new(4)]),
                (false, &[PeerId::new(3), PeerId::new(4)]),
            ],
        );

        let bitfield3 = BitField::from(&[0, 0], 9).unwrap();

        let to_download =
            picker.pick_piece(PeerId::new(9191), piece_length * 8, &bitfield3, &collector);

        assert!(to_download.is_none());

        collector.add_block(&Block {
            piece_index: 7.into(),
            index: 0.into(),
            block: vec![0; 1042].into_boxed_slice(),
        });

        let to_download =
            picker.pick_piece(PeerId::new(6), piece_length * 8, &bitfield2, &collector);

        assert_eq!(
            to_download.unwrap(),
            &[
                TaskDownload::Piece {
                    piece_index: PieceIndex(0)
                },
                TaskDownload::Piece {
                    piece_index: PieceIndex(8)
                },
                TaskDownload::BlockRange {
                    piece_index: PieceIndex(7),
                    start: BlockIndex(1042),
                    end: BlockIndex(1142)
                },
                TaskDownload::BlockRange {
                    piece_index: PieceIndex(7),
                    start: BlockIndex(1142),
                    end: BlockIndex(1242)
                },
                TaskDownload::BlockRange {
                    piece_index: PieceIndex(7),
                    start: BlockIndex(1242),
                    end: BlockIndex(1250)
                }
            ]
        );

        assert_eq_states(
            &picker.states,
            &[
                (
                    false,
                    &[
                        PeerId::new(1),
                        PeerId::new(2),
                        PeerId::new(3),
                        PeerId::new(4),
                        PeerId::new(6),
                    ],
                ),
                (false, &[PeerId::new(4)]),
                (true, &[PeerId::new(1), PeerId::new(3)]),
                (false, &[PeerId::new(4)]),
                (false, &[PeerId::new(3), PeerId::new(4)]),
                (false, &[PeerId::new(4)]),
                (true, &[PeerId::new(2), PeerId::new(3)]),
                (false, &[PeerId::new(4), PeerId::new(6)]),
                (false, &[PeerId::new(3), PeerId::new(4), PeerId::new(6)]),
            ],
        );

        println!("picker={:#?}", picker);
    }
}
