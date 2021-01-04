use std::{fs::File, sync::Arc};

use async_channel::{Receiver, RecvError, Sender};
use hashbrown::HashMap;
use kv_log_macro::info;
use tokio::runtime::Runtime;

use crate::{
    actors::peer::PeerCommand,
    fs::{FSMessage, TorrentCache},
    piece_picker::{BlockIndex, PieceIndex},
    supervisors::torrent::TorrentId,
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

        cache.iter_files_on_piece(piece, 0.into(), |fd, offset, max| {
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::runtime::Runtime;
    use smallvec::smallvec;

    use crate::{fs::FSMessage::{AddTorrent, Read, RemoveTorrent, Write}, metadata::{InfoFile::Multiple, MetaFile, MetaInfo, MetaTorrent, Torrent}, pieces::Pieces, supervisors::torrent::TorrentId};

    use super::StandardFS;

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn fs() {
        crate::logger::start();

        let runtime = Arc::new(Runtime::new().unwrap());
        let fs = StandardFS::new(runtime);

        let torrent = Torrent {
            meta: MetaTorrent {
                announce: None,
                info: MetaInfo {
                    pieces: vec![1; 20 * 110],
                    piece_length: 1000,
                    private: None,
                    files: Multiple {
                        name: "abc".to_string(),
                        files: vec![
                            MetaFile {
                                length: 98080,
                                md5sum: None,
                                path: smallvec!["a".to_string()]
                            },
                            MetaFile {
                                length: 11111,
                                md5sum: None,
                                path: smallvec!["b".to_string()]
                            },
                            MetaFile {
                                length: 198,
                                md5sum: None,
                                path: smallvec!["c".to_string()]
                            },
                            MetaFile {
                                length: 5,
                                md5sum: None,
                                path: smallvec!["d".to_string()]
                            },
                        ],
                    },
                },
                announce_list: None,
                creation_date: None,
                comment: None,
                created_by: None,
                encoding: None,
                url_list: None,
            },
            info_hash: Arc::new([])
        };

        let pieces = Pieces::from(&torrent);
        let piece_length = pieces.piece_length;
        let files_size = pieces.files_size;
        let torrent_id = TorrentId::new();

        println!("PIECES={:#?}", pieces);

        fs.try_send(AddTorrent {
            id: torrent_id,
            meta: Arc::new(torrent),
            pieces_infos: Arc::new(pieces),
        }).unwrap();

        let mut data = Vec::with_capacity(files_size);
        for _ in 0..data.capacity() {
            data.push(fastrand::u8(..));
        }

        for (index, chunk) in data.chunks(piece_length).enumerate() {
            fs.try_send(Write {
                id: torrent_id,
                piece: (index as u32).into(),
                data: Vec::from(chunk).into_boxed_slice(),
            }).unwrap();
        }

        std::thread::sleep_ms(1000);

        // for (index, n) in (0..files_size).step_by(1000).enumerate() {
        //     fs.try_send(Read {
        //         id: torrent_id,
        //         piece: 0.into(),
        //         block: 0,
        //         length: (),
        //         peer: (),
        //     }).unwrap();
        // }

    }
}
