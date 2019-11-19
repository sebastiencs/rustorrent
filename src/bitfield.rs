use crate::utils::FromSlice;
use crate::session::TorrentError;
use crate::session::Result;

#[derive(Debug)]
pub struct BitField {
    inner: Vec<u8>,
    nbits: usize
}

impl BitField {
    pub fn new(nbits: usize) -> BitField {
        BitField {
            inner: vec![0; nbits],
            nbits
        }
    }

    pub fn from(bitfield: &[u8], nbits: usize) -> Result<BitField> {
        if nbits < bitfield.len() * 8 {
            Ok(BitField {
                inner: Vec::from_slice(bitfield),
                nbits
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

            self.inner[slice_index] |= & (1 << (7 - bit_index));
        }
    }
}
