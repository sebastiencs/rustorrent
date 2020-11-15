use crate::{errors::TorrentError, supervisors::torrent::Result, utils::FromSlice};

pub enum BitFieldUpdate {
    BitField(BitField),
    Piece(usize),
}

impl From<u32> for BitFieldUpdate {
    fn from(p: u32) -> BitFieldUpdate {
        BitFieldUpdate::Piece(p as usize)
    }
}

impl From<BitField> for BitFieldUpdate {
    fn from(b: BitField) -> BitFieldUpdate {
        BitFieldUpdate::BitField(b)
    }
}

#[derive(Debug)]
pub struct BitField {
    inner: Vec<u8>,
    nbits: usize,
}

impl BitField {
    // TODO: Handle this and update
    pub fn empty() -> BitField {
        BitField {
            inner: Vec::new(),
            nbits: 0,
        }
    }

    pub fn new(nbits: usize) -> BitField {
        BitField {
            inner: vec![0; nbits],
            nbits,
        }
    }

    pub fn from(bitfield: &[u8], nbits: usize) -> Result<BitField> {
        if nbits < bitfield.len() * 8 {
            Ok(BitField {
                inner: Vec::from_slice(bitfield),
                nbits,
            })
        } else {
            Err(TorrentError::InvalidInput)
        }
    }

    pub fn get_bit(&self, index: usize) -> bool {
        if index < self.nbits {
            let slice_index = index / 8;
            let bit_index = index % 8;

            self.inner[slice_index] & (1 << (7 - bit_index)) != 0
        } else {
            false
        }
    }

    pub fn set_bit(&mut self, index: usize) {
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
                self.set_bit(piece as usize);
            }
        }
    }
}
