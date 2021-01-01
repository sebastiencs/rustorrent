use std::sync::Arc;

use async_channel::Sender;
use tokio::runtime::Runtime;

use crate::{
    actors::peer::PeerCommand,
    metadata::Torrent,
    piece_picker::{BlockIndex, PieceIndex},
    pieces::Pieces,
    supervisors::torrent::TorrentId,
};

pub mod uring_fs;

pub trait FileSystem {
    fn init(runtime: Arc<Runtime>) -> Option<Sender<FSMessage>>;
}

pub enum FSMessage {
    AddTorrent {
        id: TorrentId,
        meta: Arc<Torrent>,
        pieces_infos: Arc<Pieces>,
    },
    RemoveTorrent {
        id: TorrentId,
    },
    Read {
        id: TorrentId,
        piece: PieceIndex,
        block: BlockIndex,
        length: u32,
        peer: Sender<PeerCommand>,
    },
    Write {
        id: TorrentId,
        piece: PieceIndex,
        data: Box<[u8]>,
    },
}
