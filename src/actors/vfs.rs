use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    mem::MaybeUninit,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_channel::{Receiver, Sender};
use hashbrown::HashMap;
use kv_log_macro::{debug, info};
use tokio::runtime::Runtime;

use crate::{
    fs::FSMessage,
    metadata::{Torrent, TorrentFile},
    piece_picker::{BlockIndex, PieceIndex},
    pieces::Pieces,
    supervisors::torrent::TorrentId,
    utils::Map,
};

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
        mut fun: impl FnMut(&File, usize, usize) -> bool,
    ) {
        let (start, mut offset) = self.file_offset_at(piece, block).unwrap();

        for torrent_file in &self.files[start..] {
            let file_length = torrent_file.length as usize;
            let path = torrent_file.path.as_path();

            let file = match self.fds.get(path) {
                Some(file) => file,
                None => {
                    let file = open_file(path);
                    self.fds.insert(path.to_owned(), file);
                    self.fds.get(path).unwrap()
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

pub struct VFS {
    runtime: Arc<Runtime>,
    recv: Receiver<FSMessage>,
    torrents: Map<TorrentId, TorrentCache>,
}

impl VFS {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(runtime: Arc<Runtime>) -> Sender<FSMessage> {
        let (sender, recv) = async_channel::bounded(1000);

        let vfs = VFS {
            recv,
            runtime,
            torrents: Map::default(),
        };

        std::thread::Builder::new()
            .name("rustorrent-vfs".into())
            .spawn(move || vfs.start())
            .unwrap();

        sender
    }

    fn start(mut self) {
        while let Some(msg) = {
            self.recv
                .try_recv()
                .ok()
                .or_else(|| self.runtime.block_on(async { self.recv.recv().await.ok() }))
        } {
            self.process_msg(msg);
        }
    }

    fn process_msg(&mut self, msg: FSMessage) {
        match msg {
            FSMessage::AddTorrent {
                id,
                meta,
                pieces_infos,
            } => {
                let cache = TorrentCache {
                    files: meta.files(),
                    torrent: meta,
                    pieces_infos,
                    fds: HashMap::default(),
                };
                self.torrents.insert(id, cache);

                info!("[vfs] {:?} Add torrent", id);
            }
            FSMessage::RemoveTorrent { id } => {
                self.torrents.remove(&id);
            }
            FSMessage::Read { id, piece, .. } => {
                self.read(id, piece);
            }
            FSMessage::Write { id, piece, data } => {
                self.write(id, piece, &data);
            }
        }
    }

    fn read(&mut self, id: TorrentId, piece: PieceIndex) -> Arc<[u8]> {
        let cache = self.torrents.get_mut(&id).unwrap();

        let length = cache.pieces_infos.piece_size_of(piece) as usize;

        // TODO:
        // Change this when Arc::new_uninit_slice becomes stable
        // For now this produces the same asm code:
        // https://github.com/rust-lang/rust/issues/63291#issuecomment-680128547
        let data = std::iter::repeat_with(MaybeUninit::<u8>::uninit)
            .take(length)
            .collect::<Arc<[_]>>();
        let mut data: Arc<[u8]> = unsafe { std::mem::transmute(data) };
        let slice = Arc::get_mut(&mut data).unwrap();

        let mut cursor = 0;

        cache.iter_files_on_piece(piece, 0.into(), |mut fd, offset, max| {
            let remaining = length - cursor;
            let to_read = remaining.min(max);

            fd.seek(SeekFrom::Start(offset as u64)).unwrap();
            fd.read_exact(&mut slice[cursor..cursor + to_read]).unwrap();

            cursor += to_read;
            cursor < length
        });

        assert_eq!(cursor, length);

        data
    }

    fn write(&mut self, id: TorrentId, piece: PieceIndex, data: &[u8]) {
        let cache = self.torrents.get_mut(&id).unwrap();

        let mut data = &data[..];

        cache.iter_files_on_piece(piece, 0.into(), |mut fd, offset, max| {
            let chunk = &data[..max.min(data.len())];

            fd.seek(SeekFrom::Start(offset as u64)).unwrap();
            fd.write_all(chunk).unwrap();

            data = &data[chunk.len()..];
            !data.is_empty()
        });
    }
}
