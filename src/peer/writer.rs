use std::io::{Cursor, Write};

use byteorder::{BigEndian, WriteBytesExt};

use crate::extensions::ExtendedMessage;

use super::message::MessagePeer;

pub(crate) struct BufferWriter {
    buffer: Vec<u8>,
}

impl AsRef<[u8]> for BufferWriter {
    fn as_ref(&self) -> &[u8] {
        &self.buffer
    }
}

impl BufferWriter {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
        }
    }

    pub fn write_msg(&mut self, msg: MessagePeer<'_>) {
        self.buffer.clear();
        let mut cursor = Cursor::new(&mut self.buffer);

        match msg {
            MessagePeer::Choke => {
                cursor.write_u32::<BigEndian>(1).unwrap();
                cursor.write_u8(0).unwrap();
            }
            MessagePeer::UnChoke => {
                cursor.write_u32::<BigEndian>(1).unwrap();
                cursor.write_u8(1).unwrap();
            }
            MessagePeer::Interested => {
                cursor.write_u32::<BigEndian>(1).unwrap();
                cursor.write_u8(2).unwrap();
            }
            MessagePeer::NotInterested => {
                cursor.write_u32::<BigEndian>(1).unwrap();
                cursor.write_u8(3).unwrap();
            }
            MessagePeer::Have { piece_index } => {
                cursor.write_u32::<BigEndian>(5).unwrap();
                cursor.write_u8(4).unwrap();
                cursor.write_u32::<BigEndian>(piece_index.into()).unwrap();
            }
            MessagePeer::BitField(bitfield) => {
                cursor
                    .write_u32::<BigEndian>(1 + bitfield.len() as u32)
                    .unwrap();
                cursor.write_u8(5).unwrap();
                cursor.write_all(bitfield).unwrap();
            }
            MessagePeer::Request {
                piece: index,
                block: begin,
                length,
            } => {
                cursor.write_u32::<BigEndian>(13).unwrap();
                cursor.write_u8(6).unwrap();
                cursor.write_u32::<BigEndian>(index.into()).unwrap();
                cursor.write_u32::<BigEndian>(begin.into()).unwrap();
                cursor.write_u32::<BigEndian>(length).unwrap();
            }
            MessagePeer::Piece { piece, block, data } => {
                cursor
                    .write_u32::<BigEndian>(9 + data.len() as u32)
                    .unwrap();
                cursor.write_u8(7).unwrap();
                cursor.write_u32::<BigEndian>(piece.into()).unwrap();
                cursor.write_u32::<BigEndian>(block.into()).unwrap();
                cursor.write_all(data).unwrap();
            }
            MessagePeer::Cancel {
                piece,
                block,
                length,
            } => {
                cursor.write_u32::<BigEndian>(13).unwrap();
                cursor.write_u8(8).unwrap();
                cursor.write_u32::<BigEndian>(piece.into()).unwrap();
                cursor.write_u32::<BigEndian>(block.into()).unwrap();
                cursor.write_u32::<BigEndian>(length).unwrap();
            }
            MessagePeer::Port(port) => {
                cursor.write_u32::<BigEndian>(3).unwrap();
                cursor.write_u8(9).unwrap();
                cursor.write_u16::<BigEndian>(port).unwrap();
            }
            MessagePeer::KeepAlive => {
                cursor.write_u32::<BigEndian>(0).unwrap();
            }
            MessagePeer::Extension(ExtendedMessage::Handshake { handshake }) => {
                let bytes = crate::bencode::ser::to_bytes(&handshake).unwrap();
                cursor
                    .write_u32::<BigEndian>(2 + bytes.len() as u32)
                    .unwrap();
                cursor.write_u8(20).unwrap();
                cursor.write_u8(0).unwrap();
                cursor.write_all(&bytes).unwrap();
            }
            MessagePeer::Extension(ExtendedMessage::Message { .. }) => {}
            //MessagePeer::Extension { .. } => unreachable!()
            MessagePeer::Handshake {
                info_hash,
                extern_id,
            } => {
                let mut reserved: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 0];

                reserved[5] |= 0x10; // Support Extension Protocol

                cursor.write_all(&[19]).unwrap();
                cursor.write_all(b"BitTorrent protocol").unwrap();
                cursor.write_all(&reserved[..]).unwrap();
                cursor.write_all(info_hash.as_ref()).unwrap();
                cursor.write_all(&**extern_id).unwrap();
            }
            MessagePeer::Unknown { .. } => unreachable!(),
        }
    }
}
