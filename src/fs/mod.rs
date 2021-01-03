use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_channel::{Sender, TrySendError};
use hashbrown::HashMap;
use kv_log_macro::debug;
use tokio::runtime::Runtime;

use crate::{
    actors::peer::PeerCommand,
    metadata::{Torrent, TorrentFile},
    piece_picker::{BlockIndex, PieceIndex},
    pieces::Pieces,
    supervisors::torrent::TorrentId,
};

pub mod standard_fs;
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

fn open_file(path: &Path) -> File {
    if !path.exists() {
        let create_dir = if path.is_dir() {
            Some(path)
        } else {
            path.parent()
        };

        if let Some(dir) = create_dir {
            debug!("Creating directory {:?}", dir);
            std::fs::create_dir_all(&dir).unwrap();
        };
    }

    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .read(true)
        .open(path)
        .unwrap()
}

pub struct TorrentCache {
    pub torrent: Arc<Torrent>,
    pub pieces_infos: Arc<Pieces>,
    pub files: Vec<TorrentFile>,
    pub fds: HashMap<PathBuf, File>,
}

impl TorrentCache {
    fn file_offset_at(&self, piece: PieceIndex, block: BlockIndex) -> Option<(usize, usize)> {
        let piece_index: usize = piece.into();
        let block_index: usize = block.into();
        let piece_length = self.torrent.meta.info.piece_length as usize;

        let mut offset = (piece_index * piece_length) + block_index;

        for (index, file) in self.files.iter().enumerate() {
            if offset < file.length as usize {
                return Some((index, offset));
            }
            offset -= file.length as usize;
        }

        None
    }

    pub fn iter_files_on_piece(
        &mut self,
        piece: PieceIndex,
        block: BlockIndex,
        mut fun: impl FnMut(&mut File, usize, usize) -> bool,
    ) {
        let (start, mut offset) = self.file_offset_at(piece, block).unwrap();

        for torrent_file in &self.files[start..] {
            let file_length = torrent_file.length as usize;
            let path = torrent_file.path.as_path();

            let file = match self.fds.get_mut(path) {
                Some(file) => file,
                None => {
                    let file = open_file(path);
                    self.fds.insert(path.to_owned(), file);
                    self.fds.get_mut(path).unwrap()
                }
            };

            let max = file_length - offset;

            if !fun(file, offset, max) {
                return;
            }

            offset = 0;
        }
    }
}

pub(super) fn new_read_buffer(length: usize) -> Box<[u8]> {
    let mut data = Vec::with_capacity(length);
    unsafe { data.set_len(length) };
    data.into_boxed_slice()
}

pub(super) fn send_to_peer(
    runtime: &Runtime,
    peer: Sender<PeerCommand>,
    piece: PieceIndex,
    block: BlockIndex,
    data: Box<[u8]>,
) {
    let msg = PeerCommand::BlockData { piece, block, data };

    if let Err(TrySendError::Full(msg)) = peer.try_send(msg) {
        runtime.spawn(async move { peer.send(msg).await });
    }
}
