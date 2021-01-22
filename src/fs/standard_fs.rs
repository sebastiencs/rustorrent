use std::{fs::File, sync::Arc};

use async_channel::{Receiver, RecvError, Sender};
use hashbrown::HashMap;
use kv_log_macro::info;
use tokio::runtime::Runtime;

use crate::{
    fs::{FSMessage, TorrentCache},
    peer::peer::PeerCommand,
    piece_picker::{BlockIndex, PieceIndex},
    torrent::TorrentId,
    utils::Map,
};

use super::{new_read_buffer, send_to_peer};

trait FileOffset {
    fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> std::io::Result<()>;
    fn write_all_at(&mut self, buf: &[u8], offset: u64) -> std::io::Result<()>;
}

#[cfg(not(unix))]
impl FileOffset for File {
    fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};

        self.seek(SeekFrom::Start(offset))?;
        self.read_exact(buf)
    }
    fn write_all_at(&mut self, buf: &[u8], offset: u64) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};

        self.seek(SeekFrom::Start(offset))?;
        self.write_all(buf)
    }
}

// Use pread(2) and pwrite(2)
#[cfg(unix)]
impl FileOffset for File {
    fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        use std::os::unix::fs::FileExt;

        FileExt::read_exact_at(self, buf, offset)
    }
    fn write_all_at(&mut self, buf: &[u8], offset: u64) -> std::io::Result<()> {
        use std::os::unix::fs::FileExt;

        FileExt::write_all_at(self, buf, offset)
    }
}

/// File system implementation based on the standard library
/// (should work on most platforms)
pub struct StandardFS {
    runtime: Arc<Runtime>,
    recv: Receiver<FSMessage>,
    torrents: Map<TorrentId, TorrentCache>,
}

impl StandardFS {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(runtime: Arc<Runtime>) -> Sender<FSMessage> {
        let (sender, recv) = async_channel::bounded(1000);

        let vfs = StandardFS {
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

    fn wait_for_message(&self) -> Result<FSMessage, RecvError> {
        if let Ok(msg) = self.recv.try_recv() {
            return Ok(msg);
        };

        self.runtime.block_on(async { self.recv.recv().await })
    }

    fn start(mut self) {
        while let Ok(msg) = self.wait_for_message() {
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
            FSMessage::Read {
                id,
                piece,
                block,
                length,
                peer,
            } => {
                self.read(id, piece, block, length, peer);
            }
            FSMessage::Write { id, piece, data } => {
                self.write(id, piece, &data);
            }
        }
    }

    fn read(
        &mut self,
        id: TorrentId,
        piece: PieceIndex,
        block: BlockIndex,
        length: u32,
        peer: Sender<PeerCommand>,
    ) {
        let cache = self.torrents.get_mut(&id).unwrap();
        let length = length as usize;

        let mut data = new_read_buffer(length);
        let slice = &mut data[..];
        let mut cursor = 0;

        cache.iter_files_on_piece(piece, block, |fd, offset, max| {
            let remaining = length - cursor;
            let to_read = remaining.min(max);

            fd.read_exact_at(&mut slice[cursor..cursor + to_read], offset as u64)
                .unwrap();

            cursor += to_read;
            cursor < length
        });

        assert_eq!(cursor, length);

        send_to_peer(&self.runtime, peer, piece, block, data);
    }

    fn write(&mut self, id: TorrentId, piece: PieceIndex, data: &[u8]) {
        let cache = self.torrents.get_mut(&id).unwrap();

        let mut data = &data[..];

        cache.iter_files_on_piece(piece, 0.into(), |ref mut fd, offset, max| {
            let chunk = &data[..max.min(data.len())];

            fd.write_all_at(chunk, offset as u64).unwrap();

            data = &data[chunk.len()..];
            !data.is_empty()
        });

        assert!(data.is_empty());
    }
}
