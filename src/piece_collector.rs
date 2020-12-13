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

    fn next_empty_range(&self, at: BlockIndex) -> Option<Range<u32>> {
        let at: u32 = at.into();

        if at >= self.piece_length {
            return None;
        }

        match self.ranges.binary_search_by(|r| {
            if r.contains(&at) {
                Ordering::Equal
            } else {
                (r.start..r.end).cmp(at..at + 1)
            }
        }) {
            Ok(index) => {
                let current = self.ranges.get(index).unwrap();
                match self.ranges.get(index + 1) {
                    Some(next) => {
                        return Some(current.end..next.start);
                    }
                    None => {
                        if current.end != self.piece_length {
                            return Some(current.end..self.piece_length);
                        }
                    }
                }
            }
            Err(index) => {
                if index == 0 {
                    match self.ranges.get(0) {
                        Some(next) => return Some(0..next.start),
                        None => return Some(0..self.piece_length),
                    }
                }

                let index = index.saturating_sub(1);

                match (self.ranges.get(index), self.ranges.get(index + 1)) {
                    (Some(before), Some(after)) => return Some(before.end..after.start),
                    (Some(before), None) => {
                        return Some(before.end..self.piece_length);
                    }
                    _ => unreachable!(),
                }
            }
        }

        None
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
    pub block: Box<[u8]>,
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

    fn next_empty_range(&self, start_at: BlockIndex) -> Option<Range<u32>> {
        self.blocks_completed.next_empty_range(start_at)
    }

    fn take_piece(self) -> Box<[u8]> {
        self.piece
    }
}

pub struct IterEmptyRanges<'c> {
    start_at: BlockIndex,
    block_size: u32,
    piece_length: u32,
    piece_metadata: Option<&'c PieceMetadata>,
}

impl Iterator for IterEmptyRanges<'_> {
    type Item = Range<u32>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut next = match self.piece_metadata {
            Some(m) => m.next_empty_range(self.start_at),
            _ => {
                if self.piece_length == u32::from(self.start_at) {
                    None
                } else {
                    Some(self.start_at.into()..self.piece_length)
                }
            }
        };

        if let Some(next) = &mut next {
            let start = next.start.max(self.start_at.into());
            let end = next.end.min(start + self.block_size);

            self.start_at = end.into();
            next.start = start;
            next.end = end;

            assert!(end > start && end <= self.piece_length);
        };

        next
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

    pub fn is_empty(&self, piece_index: PieceIndex) -> bool {
        match self.pieces.get(&piece_index) {
            Some(m) => m.blocks_completed.ranges.is_empty(),
            _ => true,
        }
    }

    pub fn iter_empty_ranges(&self, piece_index: PieceIndex) -> IterEmptyRanges {
        IterEmptyRanges {
            start_at: 0.into(),
            block_size: self.pieces_infos.block_size,
            piece_length: self.pieces_infos.piece_size_of(piece_index),
            piece_metadata: self.pieces.get(&piece_index),
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
    fn range_empty() {
        let mut range = PieceRanges::new(100);

        println!("range={:?}", range);

        range.insert(5..10);
        range.insert(20..30);
        println!("range={:?}", range);

        assert_eq!(range.next_empty_range(15.into()), Some(10..20));

        assert_eq!(range.next_empty_range(10.into()), Some(10..20));

        assert_eq!(range.next_empty_range(40.into()), Some(30..100));

        assert_eq!(range.next_empty_range(20.into()), Some(30..100));

        assert_eq!(range.next_empty_range(30.into()), Some(30..100));

        assert_eq!(range.next_empty_range(0.into()), Some(0..5));

        assert_eq!(range.next_empty_range(4.into()), Some(0..5));

        assert_eq!(range.next_empty_range(5.into()), Some(10..20));

        assert_eq!(range.next_empty_range(100.into()), None);

        range.insert(90..100);
        println!("range={:?}", range);

        assert_eq!(range.next_empty_range(91.into()), None);

        let range = PieceRanges::new(100);

        assert_eq!(range.next_empty_range(91.into()), Some(0..100));

        assert_eq!(range.next_empty_range(100.into()), None);
    }

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
