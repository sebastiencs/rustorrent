use std::{cmp::Ordering, fmt::Debug, ops::Range, sync::Arc};

use crate::{
    piece_picker::{BlockIndex, PieceIndex},
    pieces::Pieces,
    utils::Map,
};

#[derive(Debug)]
struct PieceRanges {
    piece_length: u32,
    ranges: Vec<Range<u32>>,
}

impl PieceRanges {
    fn new(piece_length: u32) -> Self {
        Self {
            piece_length,
            ranges: vec![],
        }
    }

    fn is_completed(&self) -> bool {
        if self.ranges.len() != 1 {
            return false;
        }

        let range = &self.ranges[0];
        range.start == 0 && range.end == self.piece_length
    }

    fn insert<R: Into<Range<u32>>>(&mut self, block: R) {
        let range: Range<u32> = block.into();

        assert!(range.end <= self.piece_length);

        // println!("insert called with {:?}", range);
        let range_start = range.start.saturating_sub(1);

        match self.ranges.binary_search_by(|r| {
            if r.contains(&range_start) || r.contains(&range.end) {
                Ordering::Equal
            } else {
                (r.start..r.end).cmp(range.start..range.end)
            }
        }) {
            Ok(index) => {
                // println!("Ok Index={:?} obj={:?}", index, self.ranges.get(index));

                let (start, end) = {
                    let r = &mut self.ranges[index];
                    r.start = r.start.min(range.start);
                    r.end = r.end.max(range.end);
                    (r.start, r.end)
                };

                while let Some(range) = self.ranges.get(index + 1) {
                    if range.start <= end {
                        self.ranges[index].end = end.max(range.end);
                        self.ranges.remove(index + 1);
                        continue;
                    }
                    break;
                }

                if index > 0 {
                    if let Some(range) = self.ranges.get_mut(index - 1) {
                        if range.end == start {
                            range.end = end;
                            self.ranges.remove(index);
                            return;
                        }
                    }
                }
            }
            Err(index) => {
                let end = range.end;

                // println!("Err Index={:?} obj={:?}", index, self.ranges.get(index));
                self.ranges.insert(index, range);

                while self
                    .ranges
                    .get(index + 1)
                    .map(|r| r.end < end)
                    .unwrap_or(false)
                {
                    self.ranges.remove(index + 1);
                }

                while let Some(range) = self.ranges.get(index + 1) {
                    if range.start <= end {
                        self.ranges[index].end = end.max(range.end);
                        self.ranges.remove(index + 1);
                        continue;
                    }
                    break;
                }

                // index
            }
        };
    }
}

pub struct Block {
    pub piece_index: PieceIndex,
    pub index: BlockIndex,
    block: Box<[u8]>,
}

impl<'b> From<(PieceIndex, BlockIndex, &'b [u8])> for Block {
    fn from(data: (PieceIndex, BlockIndex, &'b [u8])) -> Self {
        Self {
            piece_index: data.0,
            index: data.1,
            block: Vec::from(data.2).into_boxed_slice(),
        }
    }
}

impl From<&Block> for Range<u32> {
    fn from(block: &Block) -> Self {
        let start: u32 = block.index.into();
        let end = start + block.block.len() as u32;

        start..end
    }
}

impl From<&Block> for Range<usize> {
    fn from(block: &Block) -> Self {
        let range: Range<u32> = block.into();

        range.start as usize..range.end as usize
    }
}

impl Debug for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Block")
            .field("piece_index", &self.piece_index)
            .field("index", &self.index)
            .field("block_length", &self.block.len())
            .finish()
    }
}

struct PieceMetadata {
    /// The piece data
    piece: Box<[u8]>,
    /// Value containing which parts of the piece is present
    blocks_completed: PieceRanges,
}

impl PieceMetadata {
    fn new(piece_length: u32, block: &Block) -> Self {
        let mut meta = Self {
            piece: vec![0; piece_length as usize].into_boxed_slice(),
            blocks_completed: PieceRanges::new(piece_length),
        };

        meta.add_block(block);
        meta
    }

    fn add_block(&mut self, block: &Block) -> bool {
        if let Some(bytes) = self.piece.get_mut(Range::<usize>::from(block)) {
            bytes.copy_from_slice(&block.block);
            self.blocks_completed.insert(block);
            self.blocks_completed.is_completed()
        } else {
            panic!(
                "ERROR on add_block block_index={:?} length={:?}",
                block.index,
                block.block.len()
            );
        }
    }

    fn take_piece(self) -> Box<[u8]> {
        self.piece
    }
}

pub struct PieceCollector {
    pieces: Map<PieceIndex, PieceMetadata>,
    pieces_infos: Arc<Pieces>,
}

impl PieceCollector {
    pub fn new(pieces_infos: &Arc<Pieces>) -> Self {
        Self {
            pieces: Map::default(),
            pieces_infos: Arc::clone(pieces_infos),
        }
    }

    pub fn add_block(&mut self, block: &Block) -> Option<Box<[u8]>> {
        let piece_index = block.piece_index;
        let piece_length = self.pieces_infos.piece_size_of(block.piece_index);
        let mut is_completed = false;

        self.pieces
            .entry(block.piece_index)
            .and_modify(|metadata| {
                is_completed = metadata.add_block(block);
            })
            .or_insert_with(|| PieceMetadata::new(piece_length, block));

        if is_completed {
            return self
                .pieces
                .remove(&piece_index)
                .map(PieceMetadata::take_piece);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::PieceRanges;

    #[test]
    fn range() {
        let mut range = PieceRanges::new(100);

        println!("range={:?}", range);

        range.insert(0..10);
        println!("range={:?}", range);

        range.insert(15..20);
        println!("range={:?}", range);

        range.insert(2..9);
        println!("range={:?}", range);

        assert_eq!(range.ranges, &[(0..10), (15..20)]);

        range.insert(9..12);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(0..12), (15..20)]);

        range.insert(25..30);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(0..12), (15..20), (25..30)]);

        range.insert(12..14);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(0..14), (15..20), (25..30)]);

        range.insert(23..25);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(0..14), (15..20), (23..30)]);

        range.insert(20..23);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(0..14), (15..30)]);

        range.insert(50..55);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(0..14), (15..30), (50..55)]);

        range.insert(49..60);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(0..14), (15..30), (49..60)]);

        range.ranges.remove(0);
        assert_eq!(range.ranges, &[(15..30), (49..60)]);

        range.insert(19..65);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(15..65)]);

        range.insert(67..68);
        range.insert(69..71);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(15..65), (67..68), (69..71)]);

        range.insert(17..75);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(15..75)]);

        range.ranges = vec![(15..65), (67..68), (69..71)];

        range.insert(12..70);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(12..71)]);

        range.ranges = vec![(15..65), (67..68), (69..71)];

        range.insert(16..70);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(15..71)]);

        range.ranges = vec![(15..65), (67..68), (69..71)];

        range.insert(66..67);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(15..65), (66..68), (69..71)]);

        range.insert(65..66);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(15..68), (69..71)]);

        range.ranges = vec![(15..65), (66..67), (69..71)];

        range.insert(65..69);
        println!("range={:?}", range);
        assert_eq!(range.ranges, &[(15..71)]);

        assert_eq!(range.is_completed(), false);

        range.insert(0..100);
        assert_eq!(range.is_completed(), true);
    }
}
