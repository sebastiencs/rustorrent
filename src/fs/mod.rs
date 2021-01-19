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
    metadata::{Torrent, TorrentFile},
    peer::peer::PeerCommand,
    piece_picker::{BlockIndex, PieceIndex},
    pieces::Pieces,
    supervisors::torrent::TorrentId,
};

pub mod standard_fs;
#[cfg(target_os = "linux")]
pub mod uring_fs;

pub trait FileSystem {
    fn init(runtime: Arc<Runtime>) -> Option<Sender<FSMessage>>;
}

#[cfg(not(target_os = "linux"))]
pub mod uring_fs {
    pub struct UringFS;
    impl super::FileSystem for UringFS {
        fn init(_: super::Arc<super::Runtime>) -> Option<super::Sender<super::FSMessage>> {
            None
        }
    }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_channel::Sender;
    use smallvec::smallvec;
    use tokio::runtime::Runtime;

    use crate::{
        fs::FSMessage::{AddTorrent, Read, RemoveTorrent, Write},
        metadata::{InfoFile::Multiple, MetaFile, MetaInfo, MetaTorrent, Torrent},
        peer::peer::PeerCommand,
        pieces::Pieces,
        supervisors::torrent::TorrentId,
    };

    use super::{standard_fs::StandardFS, uring_fs::UringFS, FSMessage, FileSystem};

    fn read_write(fs: Sender<FSMessage>, dir_name: &str) {
        crate::logger::start();

        let torrent = Torrent {
            meta: MetaTorrent {
                announce: None,
                info: MetaInfo {
                    pieces: vec![1; 20 * 110],
                    piece_length: 1000,
                    private: None,
                    files: Multiple {
                        name: dir_name.to_string(),
                        files: vec![
                            MetaFile {
                                length: 98080,
                                md5sum: None,
                                path: smallvec!["a".to_string()],
                            },
                            MetaFile {
                                length: 11111,
                                md5sum: None,
                                path: smallvec!["b".to_string()],
                            },
                            MetaFile {
                                length: 198,
                                md5sum: None,
                                path: smallvec!["c".to_string()],
                            },
                            MetaFile {
                                length: 5,
                                md5sum: None,
                                path: smallvec!["d".to_string()],
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
            info_hash: Arc::new([]),
        };

        let pieces = Pieces::from(&torrent);
        let pieces_clone = pieces.clone();
        let piece_length = pieces.piece_length;
        let files_size = pieces.files_size;
        let torrent_id = TorrentId::new();

        println!("PIECES={:#?}", pieces);

        fs.try_send(AddTorrent {
            id: torrent_id,
            meta: Arc::new(torrent),
            pieces_infos: Arc::new(pieces_clone),
        })
        .unwrap();

        let mut data = Vec::with_capacity(files_size);
        for _ in 0..data.capacity() {
            data.push(fastrand::u8(..));
        }

        for (index, chunk) in data.chunks(piece_length).enumerate() {
            fs.try_send(Write {
                id: torrent_id,
                piece: (index as u32).into(),
                data: Vec::from(chunk).into_boxed_slice(),
            })
            .unwrap();
        }

        std::thread::sleep(std::time::Duration::from_millis(500));

        let (sender, recv) = async_channel::unbounded();

        for piece in 0..pieces.num_pieces {
            if piece == pieces.num_pieces - 1 {
                fs.try_send(Read {
                    id: torrent_id,
                    piece: (piece as u32).into(),
                    block: 0.into(),
                    length: pieces.last_piece_length as u32,
                    peer: sender.clone(),
                })
                .unwrap();
            } else {
                fs.try_send(Read {
                    id: torrent_id,
                    piece: (piece as u32).into(),
                    block: 0.into(),
                    length: 500,
                    peer: sender.clone(),
                })
                .unwrap();

                fs.try_send(Read {
                    id: torrent_id,
                    piece: (piece as u32).into(),
                    block: 500.into(),
                    length: 500,
                    peer: sender.clone(),
                })
                .unwrap();
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(500));

        let mut read = vec![0; data.len()];

        while let Ok(PeerCommand::BlockData { piece, block, data }) = recv.try_recv() {
            let piece: usize = piece.into();
            let block: usize = block.into();

            let offset = (piece * pieces.piece_length) + block;
            read[offset..offset + data.len()].copy_from_slice(&data);
        }

        assert_eq!(data.len(), read.len());
        assert_eq!(data, read);

        // Check 1st file
        let mut vec = Vec::new();
        let mut file = std::fs::File::open(dir_name.to_string() + "/a").unwrap();
        //let mut file = std::fs::File::open("abc/a").unwrap();
        std::io::Read::read_to_end(&mut file, &mut vec).unwrap();
        assert_eq!(vec, &data[..98080]);

        // Check 2nd file
        let mut vec = Vec::new();
        let mut file = std::fs::File::open(dir_name.to_string() + "/b").unwrap();
        // let mut file = std::fs::File::open("abc/b").unwrap();
        std::io::Read::read_to_end(&mut file, &mut vec).unwrap();
        assert_eq!(vec, &data[98080..98080 + 11111]);

        // Check 3rd file
        let mut vec = Vec::new();
        let mut file = std::fs::File::open(dir_name.to_string() + "/c").unwrap();
        // let mut file = std::fs::File::open("abc/c").unwrap();
        std::io::Read::read_to_end(&mut file, &mut vec).unwrap();
        assert_eq!(vec, &data[98080 + 11111..98080 + 11111 + 198]);

        // Check 4th file
        let mut vec = Vec::new();
        let mut file = std::fs::File::open(dir_name.to_string() + "/d").unwrap();
        // let mut file = std::fs::File::open("abc/d").unwrap();
        std::io::Read::read_to_end(&mut file, &mut vec).unwrap();
        assert_eq!(vec, &data[98080 + 11111 + 198..]);

        fs.try_send(RemoveTorrent { id: torrent_id }).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support tokio's runtime
    fn standard_fs() {
        crate::logger::start();

        std::fs::remove_dir_all("abc").ok();

        let runtime = Arc::new(Runtime::new().unwrap());
        let fs = StandardFS::new(runtime);

        read_write(fs, "abc");
        std::fs::remove_dir_all("abc").ok();
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn uring_fs() {
        crate::logger::start();

        std::fs::remove_dir_all("aaa").ok();

        let runtime = Arc::new(Runtime::new().unwrap());
        let fs = match UringFS::init(runtime) {
            Some(fs) => fs,
            _ => return, // io_uring not supported
        };

        read_write(fs, "aaa");
        std::fs::remove_dir_all("aaa").ok();
    }
}
