use std::{
    io::{Cursor, Result, Write},
    task::{Context, Poll},
};

use byteorder::{BigEndian, WriteBytesExt};
use tokio::net::TcpStream;

use crate::extensions::ExtendedMessage;

use super::message::MessagePeer;

pub trait TryWrite {
    fn try_write(&self, _: &[u8]) -> Result<usize>;
    fn poll_writable(&self, cx: &mut Context<'_>) -> Poll<Result<()>>;
}

impl TryWrite for TcpStream {
    fn try_write(&self, buffer: &[u8]) -> Result<usize> {
        tokio::net::TcpStream::try_write(self, buffer)
    }

    fn poll_writable(&self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        tokio::net::TcpStream::poll_write_ready(self, cx)
    }
}

pub(crate) struct BufferWriter {
    buffer: Vec<u8>,
    pos: usize,
}

impl AsRef<[u8]> for BufferWriter {
    fn as_ref(&self) -> &[u8] {
        &self.buffer[..self.pos]
    }
}

impl BufferWriter {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
            pos: 0,
        }
    }

    pub fn consume(&mut self, nbytes: usize) {
        if nbytes == self.pos {
            self.pos = 0;
        } else {
            assert!(nbytes < self.pos);
            self.buffer.copy_within(nbytes..self.pos, 0);
            self.pos -= nbytes;
        }
    }

    pub fn len(&self) -> usize {
        self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.pos == 0
    }

    pub fn write_msg<'a, M>(&mut self, msg: M)
    where
        M: Into<MessagePeer<'a>>,
    {
        let msg = msg.into();

        let mut cursor = Cursor::new(&mut self.buffer);

        cursor.set_position(self.pos as u64);

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
            MessagePeer::Extension(ExtendedMessage::Message { id, buffer }) => {
                cursor
                    .write_u32::<BigEndian>(2 + buffer.len() as u32)
                    .unwrap();
                cursor.write_u8(20).unwrap();
                cursor.write_u8(id).unwrap();
                cursor.write_all(&buffer).unwrap();
            }
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

        self.pos = cursor.position() as usize;
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        extensions::ExtendedHandshake,
        peer::{message::MessagePeer, peer::PeerExternId},
    };

    use super::BufferWriter;

    #[test]
    #[should_panic]
    fn unknown() {
        let mut buffer = BufferWriter::new(2);
        buffer.write_msg(MessagePeer::Unknown {
            id: 1,
            buffer: &[1],
        });
    }

    #[test]
    #[should_panic]
    fn consume_more() {
        let mut buffer = BufferWriter::new(2);
        buffer.write_msg(MessagePeer::KeepAlive);
        buffer.consume(10);
    }

    #[test]
    fn writer() {
        let mut buffer = BufferWriter::new(2);
        assert!(buffer.is_empty());

        buffer.write_msg(MessagePeer::KeepAlive);
        assert_eq!(buffer.as_ref(), &[0, 0, 0, 0]);

        buffer.consume(1);
        assert_eq!(buffer.as_ref(), &[0, 0, 0]);

        buffer.write_msg(MessagePeer::Choke);
        assert_eq!(buffer.as_ref(), &[0, 0, 0, 0, 0, 0, 1, 0]);

        buffer.consume(4);
        assert_eq!(buffer.as_ref(), &[0, 0, 1, 0]);
        buffer.consume(4);
        assert_eq!(buffer.as_ref().len(), 0);

        buffer.write_msg(MessagePeer::UnChoke);
        assert_eq!(buffer.as_ref(), &[0, 0, 0, 1, 1]);
        buffer.consume(5);

        buffer.write_msg(MessagePeer::Interested);
        assert_eq!(buffer.as_ref(), &[0, 0, 0, 1, 2]);
        buffer.consume(5);

        buffer.write_msg(MessagePeer::NotInterested);
        assert_eq!(buffer.as_ref(), &[0, 0, 0, 1, 3]);
        buffer.consume(5);
        assert!(buffer.is_empty());

        buffer.write_msg(MessagePeer::Have {
            piece_index: 0xB11FF0F0.into(),
        });
        assert_eq!(buffer.as_ref(), &[0, 0, 0, 5, 4, 177, 31, 240, 240]);
        buffer.consume(9);

        buffer.write_msg(MessagePeer::BitField(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]));
        assert_eq!(
            buffer.as_ref(),
            &[0, 0, 0, 12, 5, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]
        );
        buffer.consume(buffer.len());

        buffer.write_msg(MessagePeer::Request {
            piece: 1.into(),
            block: 0xFF.into(),
            length: 0xFBFBFBFB,
        });
        assert_eq!(
            buffer.as_ref(),
            &[0, 0, 0, 13, 6, 0, 0, 0, 1, 0, 0, 0, 255, 251, 251, 251, 251]
        );
        buffer.consume(buffer.len());

        buffer.write_msg(MessagePeer::Piece {
            piece: 5.into(),
            block: 2.into(),
            data: &[1, 2, 3, 4, 5],
        });
        assert_eq!(
            buffer.as_ref(),
            &[0, 0, 0, 14, 7, 0, 0, 0, 5, 0, 0, 0, 2, 1, 2, 3, 4, 5]
        );
        buffer.consume(buffer.len());

        buffer.write_msg(MessagePeer::Cancel {
            piece: 6.into(),
            block: 7.into(),
            length: 101,
        });
        assert_eq!(
            buffer.as_ref(),
            &[0, 0, 0, 13, 8, 0, 0, 0, 6, 0, 0, 0, 7, 0, 0, 0, 101]
        );
        buffer.consume(buffer.len());

        buffer.write_msg(MessagePeer::Port(10101));
        assert_eq!(buffer.as_ref(), &[0, 0, 0, 3, 9, 39, 117]);
        buffer.consume(buffer.len());

        buffer.write_msg(MessagePeer::Extension(
            crate::extensions::ExtendedMessage::Handshake {
                handshake: Box::new(ExtendedHandshake::default()),
            },
        ));
        assert_eq!(buffer.as_ref(), &[0, 0, 0, 4, 20, 0, 100, 101]);
        buffer.consume(buffer.len());

        buffer.write_msg(MessagePeer::Extension(
            crate::extensions::ExtendedMessage::Message {
                id: 101,
                buffer: (&[1, 2, 3]),
            },
        ));
        assert_eq!(buffer.as_ref(), &[0, 0, 0, 5, 20, 101, 1, 2, 3]);
        buffer.consume(buffer.len());

        let info_hash = (0..20).collect::<Vec<u8>>();
        let peer_id = (0..20).collect::<Vec<u8>>();
        let peer_id = PeerExternId::new(&peer_id);

        buffer.write_msg(MessagePeer::Handshake {
            info_hash: &info_hash,
            extern_id: &peer_id,
        });
        assert_eq!(
            buffer.as_ref(),
            &[
                19, 66, 105, 116, 84, 111, 114, 114, 101, 110, 116, 32, 112, 114, 111, 116, 111,
                99, 111, 108, 0, 0, 0, 0, 0, 16, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
                13, 14, 15, 16, 17, 18, 19, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
                16, 17, 18, 19
            ]
        );
        buffer.consume(buffer.len());
    }
}
