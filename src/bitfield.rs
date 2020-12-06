use crate::{
    errors::TorrentError, piece_picker::PieceIndex, supervisors::torrent::Result, utils::FromSlice,
};

pub enum BitFieldUpdate {
    BitField(BitField),
    Piece(PieceIndex),
}

impl<T: Into<PieceIndex>> From<T> for BitFieldUpdate {
    fn from(p: T) -> BitFieldUpdate {
        BitFieldUpdate::Piece(p.into())
    }
}

impl From<BitField> for BitFieldUpdate {
    fn from(b: BitField) -> BitFieldUpdate {
        BitFieldUpdate::BitField(b)
    }
}

#[derive(Debug)]
pub struct BitField {
    inner: Box<[u8]>,
    nbits: usize,
}

impl BitField {
    pub fn new(nbits: usize) -> BitField {
        BitField {
            inner: vec![0; (nbits / 8) + 1].into_boxed_slice(),
            nbits,
        }
    }

    pub fn from(bitfield: &[u8], nbits: usize) -> Result<BitField> {
        if nbits < bitfield.len() * 8 {
            Ok(BitField {
                inner: Vec::from_slice(bitfield).into_boxed_slice(),
                nbits,
            })
        } else {
            Err(TorrentError::InvalidInput)
        }
    }

    pub fn get_bit<I: Into<usize>>(&self, index: I) -> bool {
        let index: usize = index.into();

        if index < self.nbits {
            let slice_index = index / 8;
            let bit_index = index % 8;

            self.inner[slice_index] & (1 << (7 - bit_index)) != 0
        } else {
            false
        }
    }

    pub fn set_bit<I: Into<usize>>(&mut self, index: I) {
        let index: usize = index.into();

        if index < self.nbits {
            let slice_index = index / 8;
            let bit_index = index % 8;

            self.inner[slice_index] |= &(1 << (7 - bit_index));
        }
    }

    pub fn update(&mut self, update: BitFieldUpdate) {
        match update {
            BitFieldUpdate::BitField(bitfield) => {
                *self = bitfield;
            }
            BitFieldUpdate::Piece(piece) => {
                self.set_bit(piece);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BitField;

    #[test]
    fn size() {
        let bitfield = BitField::new(12);
        println!("bitfield={:?}", bitfield);
    }
}
