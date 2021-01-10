use std::{convert::TryFrom, io::Cursor};

use byteorder::{BigEndian, ReadBytesExt};

use crate::{
    errors::TorrentError,
    extensions::{ExtendedHandshake, ExtendedMessage},
    peer::peer::PeerExternId,
    piece_picker::{BlockIndex, PieceIndex},
    supervisors::torrent::Result,
};

#[derive(Debug)]
pub enum MessagePeer<'a> {
    KeepAlive,
    Choke,
    UnChoke,
    Interested,
    NotInterested,
    Have {
        piece_index: PieceIndex,
    },
    BitField(&'a [u8]),
    Request {
        piece: PieceIndex,
        block: BlockIndex,
        length: u32,
    },
    Piece {
        piece: PieceIndex,
        block: BlockIndex,
        data: &'a [u8],
    },
    Cancel {
        piece: PieceIndex,
        block: BlockIndex,
        length: u32,
    },
    Port(u16),
    Extension(ExtendedMessage<'a>),
    Handshake {
        info_hash: &'a [u8],
        extern_id: &'a PeerExternId,
    },
    Unknown {
        id: u8,
        buffer: &'a [u8],
    },
}

impl From<ExtendedHandshake> for MessagePeer<'_> {
    fn from(handshake: ExtendedHandshake) -> Self {
        MessagePeer::Extension(ExtendedMessage::Handshake {
            handshake: Box::new(handshake),
        })
    }
}

impl<'a> TryFrom<&'a [u8]> for MessagePeer<'a> {
    type Error = TorrentError;

    fn try_from(buffer: &'a [u8]) -> Result<MessagePeer> {
        if buffer.is_empty() {
            return Ok(MessagePeer::KeepAlive);
        }
        let id = buffer[0];
        let buffer = &buffer[1..];

        let mut cursor = Cursor::new(buffer);

        Ok(match id {
            0 => MessagePeer::Choke,
            1 => MessagePeer::UnChoke,
            2 => MessagePeer::Interested,
            3 => MessagePeer::NotInterested,
            4 => {
                let piece_index = cursor.read_u32::<BigEndian>()?.into();

                MessagePeer::Have { piece_index }
            }
            5 => MessagePeer::BitField(buffer),
            6 => {
                let index = cursor.read_u32::<BigEndian>()?.into();
                let begin = cursor.read_u32::<BigEndian>()?.into();
                let length = cursor.read_u32::<BigEndian>()?;

                MessagePeer::Request {
                    piece: index,
                    block: begin,
                    length,
                }
            }
            7 => {
                let index = cursor.read_u32::<BigEndian>()?.into();
                let begin = cursor.read_u32::<BigEndian>()?.into();
                let block = &buffer[8..];

                MessagePeer::Piece {
                    piece: index,
                    block: begin,
                    data: block,
                }
            }
            8 => {
                let piece = cursor.read_u32::<BigEndian>()?.into();
                let block = cursor.read_u32::<BigEndian>()?.into();
                let length = cursor.read_u32::<BigEndian>()?;

                MessagePeer::Cancel {
                    piece,
                    block,
                    length,
                }
            }
            9 => {
                let port = cursor.read_u16::<BigEndian>()?;

                MessagePeer::Port(port)
            }
            20 => match cursor.read_u8()? {
                0 => {
                    let handshake = crate::bencode::de::from_bytes(&buffer[1..])?;
                    MessagePeer::Extension(ExtendedMessage::Handshake {
                        handshake: Box::new(handshake),
                    })
                }
                _ => MessagePeer::Extension(ExtendedMessage::Message {
                    id,
                    buffer: &buffer[1..],
                }),
            },
            id => MessagePeer::Unknown { id, buffer },
        })
    }
}
