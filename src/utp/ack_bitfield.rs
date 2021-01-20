use super::SequenceNumber;

const BITFIELD_LENGTH: usize = 170;

const MASKS: &[u8] = &[
    0b00000000, 0b00000001, 0b00000011, 0b00000111, 0b00001111, 0b00011111, 0b00111111,
    0b01111111,
    // 0b11111111,
];

// Here is the layout of a bitmask representing the first 32 packet
// acks represented in a selective ACK bitfield:
// 0               8               16
// +---------------+---------------+---------------+---------------+
// | 9 8 ...   3 2 | 17   ...   10 | 25   ...   18 | 33   ...   26 |
// +---------------+---------------+---------------+---------------+

pub(crate) struct AckedBitfield {
    first: Option<SequenceNumber>,
    last: SequenceNumber,
    bitfield: [u8; BITFIELD_LENGTH],
}

impl std::fmt::Debug for AckedBitfield {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("AckedBitfield")
            .field("first", &self.first.map(|n| n.0))
            .field("last", &self.last.0)
            .finish()?;

        for byte in &self.bitfield[..5] {
            write!(f, " {:08b}", byte)?;
        }

        Ok(())
    }
}

impl Default for AckedBitfield {
    fn default() -> Self {
        AckedBitfield {
            first: None,
            last: 0.into(),
            bitfield: [0; BITFIELD_LENGTH],
        }
    }
}

impl AckedBitfield {
    pub(crate) fn ack(&mut self, num: SequenceNumber) {
        if num.cmp_greater(self.last) {
            self.last = num;
        }

        match self.first {
            Some(first) if num == first + 1 => {
                let prev_first = first;

                self.first.replace(num);
                let shift = self.find_shift_number();
                if shift > 0 {
                    self.first.replace(num + shift as u16 - 1);
                    self.shift(shift, prev_first);
                }
            }
            Some(first) if num.cmp_greater(first) => {
                let distance: u16 = (num - first).into();
                let distance = distance - 2;

                let index = (distance / 8) as usize;
                let bit = distance % 8;

                if let Some(byte) = self.bitfield.get_mut(index) {
                    *byte |= &(1 << bit);
                };
            }
            None => {
                self.first.replace(num);
            }
            Some(_) => {}
        }
    }

    pub(crate) fn init(&mut self, num: SequenceNumber) {
        assert!(self.first.is_none());

        self.first.replace(num);
        self.last = num;
    }

    pub(crate) fn bytes_for_packet(&self) -> Option<&[u8]> {
        let first = self.first.unwrap();

        if first == self.last {
            return None;
        }

        let distance: u16 = (self.last - first).into();
        let distance = distance - 2;
        let last_index = (distance / 8) as usize;

        Some(&self.bitfield[..=last_index])
    }

    pub(crate) fn current(&self) -> SequenceNumber {
        self.first.unwrap()
    }

    fn find_shift_number(&self) -> usize {
        for (index, byte) in self.bitfield.iter().enumerate() {
            let byte = *byte;
            if byte == 0b11111111 {
                continue;
            }
            return (index * 8) + byte.trailing_ones() as usize + 1;
        }
        panic!("error in shifting");
    }

    fn shift(&mut self, mut n: usize, prev_first: SequenceNumber) {
        if n >= 8 {
            let nmove = n / 8;
            self.bitfield.copy_within(nmove.., 0);

            // TODO: Use core::slice::fill when stable
            self.bitfield
                .iter_mut()
                .rev()
                .take(nmove)
                .for_each(|b| *b = 0);

            n %= 8;
        }

        if n == 0 {
            return;
        }

        assert!(n < 8);

        // println!("LA n={} last={:?} prev_first={:?}", n, self.last, prev_first);

        let distance: u16 = (self.last - prev_first).into();
        let distance = distance.max(2) - 2;
        let mask = MASKS[n];
        let shift_prev = 8 - n;

        let last_index = (distance / 8) as usize;
        let mut prev = 0;

        for byte in self.bitfield[..=last_index].iter_mut().rev() {
            let tmp = *byte & mask;
            *byte = (*byte >> n) | (prev << shift_prev);
            prev = tmp;
        }
    }

    #[cfg(test)]
    fn reset(&mut self) {
        self.bitfield.iter_mut().for_each(|b| *b = 0);
    }

    #[cfg(test)]
    fn ack_numbers(&self) -> Vec<usize> {
        let first = self.first.unwrap().0 as usize;
        let mut vec = vec![first];

        for (index, byte) in self.bitfield[..5].iter().enumerate() {
            for bit in 0..8 {
                if byte & (1 << bit) != 0 {
                    vec.push(first + (index * 8) + bit + 2);
                }
            }
        }

        vec
    }
}

#[cfg(test)]
mod tests {
    use super::{AckedBitfield, SequenceNumber, BITFIELD_LENGTH};

    #[test]
    fn acked_bitfield_acks() {
        let mut bitfield = AckedBitfield {
            first: Some(SequenceNumber::from(1)),
            last: SequenceNumber::from(10),
            bitfield: [0; BITFIELD_LENGTH],
        };

        bitfield.ack(2.into());

        assert_eq!(bitfield.first.unwrap().0, 2);
        assert_eq!(bitfield.last.0, 10);
        assert_eq!(bitfield.bitfield[0], 0b00000000);

        bitfield.ack(4.into());

        assert_eq!(bitfield.first.unwrap().0, 2);
        assert_eq!(bitfield.last.0, 10);
        assert_eq!(bitfield.bitfield[0], 0b00000001);

        for n in 7..11 {
            bitfield.ack(n.into());
        }

        assert_eq!(bitfield.first.unwrap().0, 2);
        assert_eq!(bitfield.last.0, 10);
        assert_eq!(bitfield.bitfield[0], 0b01111001);

        for n in 15..30 {
            bitfield.ack(n.into());
        }

        assert_eq!(bitfield.first.unwrap().0, 2);
        assert_eq!(bitfield.last.0, 29);
        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b01111001, 0b11111000, 0b11111111, 0b00000011, 0b00000000]
        );
        assert_eq!(
            &bitfield.ack_numbers(),
            &[2, 4, 7, 8, 9, 10, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29]
        );

        bitfield.ack(3.into());

        assert_eq!(bitfield.first.unwrap().0, 4);
        assert_eq!(bitfield.last.0, 29);
        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b00011110, 0b11111110, 0b11111111, 0b00000000, 0b00000000]
        );
        assert_eq!(
            &bitfield.ack_numbers(),
            &[4, 7, 8, 9, 10, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29]
        );

        for n in 39..42 {
            bitfield.ack(n.into());
        }

        assert_eq!(bitfield.last.0, 41);
        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b00011110, 0b11111110, 0b11111111, 0b00000000, 0b00001110]
        );

        bitfield.ack(5.into());

        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b00001111, 0b11111111, 0b01111111, 0b00000000, 0b00000111]
        );
        assert_eq!(
            &bitfield.ack_numbers(),
            &[
                5, 7, 8, 9, 10, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 39, 40,
                41
            ]
        );

        bitfield.ack(5.into());
        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b00001111, 0b11111111, 0b01111111, 0b00000000, 0b00000111]
        );

        bitfield.ack(6.into());

        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b11111000, 0b11111111, 0b00000011, 0b00111000, 0b00000000]
        );
        assert_eq!(
            &bitfield.ack_numbers(),
            &[10, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 39, 40, 41]
        );

        bitfield.ack(11.into());
        bitfield.ack(12.into());
        bitfield.ack(13.into());

        assert_eq!(bitfield.first.unwrap().0, 13);
        assert_eq!(bitfield.last.0, 41);
        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b11111111, 0b01111111, 0b00000000, 0b00000111, 0b00000000]
        );
        assert_eq!(
            &bitfield.ack_numbers(),
            &[13, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 39, 40, 41]
        );

        bitfield.ack(14.into());

        assert_eq!(bitfield.first.unwrap().0, 29);
        assert_eq!(bitfield.last.0, 41);
        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b00000000, 0b00000111, 0b00000000, 0b00000000, 0b00000000]
        );
        assert_eq!(&bitfield.ack_numbers(), &[29, 39, 40, 41]);
    }

    #[test]
    fn acked_bitfield_find_shift_number() {
        let mut bitfield = AckedBitfield {
            first: Some(SequenceNumber::from(1)),
            last: SequenceNumber::from(10),
            bitfield: [0; BITFIELD_LENGTH],
        };

        const VALS: &[u8] = &[
            0b00000000, 0b00000001, 0b00000011, 0b00000111, 0b00001111, 0b00011111, 0b00111111,
            0b01111111, 0b11111111,
        ];

        for big_index in 0..64 {
            for (index, val) in VALS.iter().enumerate() {
                bitfield.bitfield[big_index] = *val;
                assert_eq!(bitfield.find_shift_number(), (big_index * 8) + index + 1);
            }
        }
    }

    #[test]
    fn acked_bitfield_shift_full() {
        let mut bitfield = AckedBitfield {
            first: Some(SequenceNumber::from(1)),
            last: SequenceNumber::from(10),
            bitfield: [0; BITFIELD_LENGTH],
        };
        for n in (3..40).step_by(3) {
            bitfield.ack(SequenceNumber::from(n));
        }

        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b01001001, 0b10010010, 0b00100100, 0b01001001, 0b00010010]
        );

        bitfield.shift(8, 1.into());

        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b10010010, 0b00100100, 0b01001001, 0b00010010, 0b00000000]
        );

        bitfield.shift(11, 1.into());

        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b00100100, 0b01001001, 0b00000010, 0b00000000, 0b00000000]
        );

        bitfield.reset();

        for n in (3..40).step_by(3) {
            bitfield.ack(SequenceNumber::from(n));
            bitfield.ack(SequenceNumber::from(n + 1));
        }

        bitfield.shift(13, 1.into());

        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b01101101, 0b11011011, 0b10110110, 0b00000001, 0b00000000]
        );
    }

    #[test]
    fn acked_bitfield_shift() {
        let mut bitfield = AckedBitfield {
            first: Some(SequenceNumber::from(1)),
            last: SequenceNumber::from(10),
            bitfield: [0; BITFIELD_LENGTH],
        };
        for n in (3..40).step_by(3) {
            bitfield.ack(SequenceNumber::from(n));
        }

        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b01001001, 0b10010010, 0b00100100, 0b01001001, 0b00010010]
        );

        bitfield.shift(2, 1.into());

        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b10010010, 0b00100100, 0b01001001, 0b10010010, 0b00000100]
        );

        bitfield.shift(1, 1.into());

        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b01001001, 0b10010010, 0b00100100, 0b01001001, 0b00000010]
        );

        bitfield.shift(5, 1.into());

        assert_eq!(
            &bitfield.bitfield[..5],
            &[0b10010010, 0b00100100, 0b01001001, 0b00010010, 0b00000000]
        );

        assert_eq!(bitfield.first.unwrap().0, 1);
        assert_eq!(bitfield.last.0, 39);
    }

    #[test]
    fn acked_bitfield() {
        let mut bitfield = AckedBitfield {
            first: Some(SequenceNumber::from(1)),
            last: SequenceNumber::from(10),
            bitfield: [0; BITFIELD_LENGTH],
        };
        for n in (3..19).step_by(2) {
            bitfield.ack(SequenceNumber::from(n));
            println!("LA {:?}", bitfield);
        }

        let mut bitfield = AckedBitfield {
            first: Some(SequenceNumber::from(1)),
            last: SequenceNumber::from(10),
            bitfield: [0; BITFIELD_LENGTH],
        };
        for n in (3..19).step_by(3) {
            bitfield.ack(SequenceNumber::from(n));
            println!("LA {:?}", bitfield);
        }
    }
}
