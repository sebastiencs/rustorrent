use std::{cell::RefCell, convert::TryInto, ptr::NonNull, sync::Arc};

use async_channel::{Receiver, RecvError, Sender};
use hashbrown::HashMap;
use kv_log_macro::info;
use tokio::runtime::Runtime;

use crate::{
    actors::peer::PeerCommand,
    fs::TorrentCache,
    io_uring::file::FilesUring,
    piece_picker::{BlockIndex, PieceIndex},
    supervisors::torrent::TorrentId,
    utils::{Map, NoHash},
};

use super::{send_to_peer, FSMessage, FileSystem};

/// FileSystem implementation based on io_uring
pub struct UringFS {
    runtime: Arc<Runtime>,
    recv: Receiver<FSMessage>,
    torrents: Map<TorrentId, TorrentCache>,
    files_ring: RefCell<Box<FilesUring<NonNull<u8>>>>,
    pending_buffers: Map<NonNull<u8>, Pending>,
}

unsafe impl Send for UringFS {}

enum Pending {
    Write {
        nrequests: u32,
        buffer_length: u32,
    },
    Read {
        nrequests: u32,
        piece: PieceIndex,
        block: BlockIndex,
        buffer: Box<[u8]>,
        peer: Sender<PeerCommand>,
    },
}

impl Pending {
    fn extract_read(self) -> (PieceIndex, BlockIndex, Box<[u8]>, Sender<PeerCommand>) {
        match self {
            Pending::Read {
                nrequests: _,
                piece,
                block,
                buffer,
                peer,
            } => (piece, block, buffer, peer),
            Pending::Write { .. } => panic!(),
        }
    }
}

impl FileSystem for UringFS {
    fn init(runtime: Arc<Runtime>) -> Option<Sender<FSMessage>> {
        let (sender, recv) = async_channel::bounded(1000);

        let vfs = UringFS {
            recv,
            runtime,
            torrents: Map::default(),
            files_ring: RefCell::new(Box::new(FilesUring::new(256).ok()?)),
            pending_buffers: Map::with_capacity_and_hasher(16, NoHash::default()),
        };

        std::thread::Builder::new()
            .name("rustorrent-io-uring".into())
            .spawn(move || vfs.start())
            .unwrap();

        Some(sender)
    }
}

fn drop_box_from_ptr(ptr: NonNull<u8>, buffer_length: u32) {
    unsafe {
        let slice = std::slice::from_raw_parts_mut(ptr.as_ptr() as *mut u8, buffer_length as usize);
        // Drop the Box
        Box::from_raw(slice);
    }
}

impl UringFS {
    fn wait_for_message(&self) -> Result<FSMessage, RecvError> {
        if let Ok(msg) = self.recv.try_recv() {
            return Ok(msg);
        };

        self.runtime.block_on(async { self.recv.recv().await })
    }

    fn start(mut self) {
        // Wait for operations to supply to the kernel
        while let Ok(msg) = self.wait_for_message() {
            self.process_msg(msg);

            // Process all available messages before notifying the kernel
            while let Some(msg) = self.recv.try_recv().ok() {
                self.process_msg(msg);
            }

            let mut ring = self.files_ring.borrow_mut();
            ring.submit();

            loop {
                while let Some(completed) = ring.get_completed() {
                    let (ptr, _result) = match completed {
                        (Some(ptr), result) => (ptr, result),
                        _ => continue,
                    };

                    match self.pending_buffers.get_mut(&ptr).unwrap() {
                        Pending::Write {
                            nrequests,
                            buffer_length,
                        } => {
                            if *nrequests != 1 {
                                *nrequests -= 1;
                                continue;
                            }

                            // No more operation uses that buffer, drop it
                            drop_box_from_ptr(ptr, *buffer_length);
                            self.pending_buffers.remove(&ptr).unwrap();
                        }
                        Pending::Read { nrequests, .. } => {
                            if *nrequests != 1 {
                                *nrequests -= 1;
                                continue;
                            }

                            let pending = self.pending_buffers.remove(&ptr).unwrap();
                            let (piece, block, buffer, peer) = pending.extract_read();

                            send_to_peer(&self.runtime, peer, piece, block, buffer);
                        }
                    }
                }

                // The kernel processed all operations, or we got new
                // operations available
                if ring.in_flight() == 0 || !self.recv.is_empty() {
                    break;
                }

                // We wait for the kernel to complete all operations
                // as we prioritize running operations
                ring.block();
            }
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
                self.write(id, piece, data);
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
        let mut ring = self.files_ring.borrow_mut();
        let length = length as usize;

        let mut data = {
            let mut data = Vec::with_capacity(length);
            unsafe { data.set_len(length) };
            data.into_boxed_slice()
        };
        let user_data = NonNull::new(data.as_mut_ptr()).unwrap();

        let slice = &mut data[..];
        let mut cursor = 0;
        let mut nrequest_on_data = 0;

        cache.iter_files_on_piece(piece, block, |fd, offset, max| {
            let remaining = length - cursor;
            let to_read = remaining.min(max);

            // Safety: The buffer stays alive at least until all requests
            // completed
            unsafe {
                ring.read_with_data(fd, offset, &mut slice[cursor..cursor + to_read], user_data);
            }

            nrequest_on_data += 1;
            cursor += to_read;
            cursor < length
        });

        assert_eq!(cursor, length);
        assert!(nrequest_on_data > 0);

        let user_data = NonNull::new(data.as_mut_ptr()).unwrap();

        self.pending_buffers.insert(
            user_data,
            Pending::Read {
                nrequests: nrequest_on_data,
                peer,
                piece,
                block,
                buffer: data,
            },
        );
    }

    fn write(&mut self, id: TorrentId, piece: PieceIndex, mut data: Box<[u8]>) {
        let cache = self.torrents.get_mut(&id).unwrap();
        let mut ring = self.files_ring.borrow_mut();

        let user_data = NonNull::new(data.as_mut_ptr()).unwrap();

        let mut slice = &data[..];
        let mut nrequest_on_data = 0;

        cache.iter_files_on_piece(piece, 0.into(), |fd, offset, max| {
            let chunk = &slice[..max.min(slice.len())];

            // Safety: The buffer is dropped only after all requests completed
            unsafe {
                ring.write_with_data(fd, offset, chunk, user_data);
            }

            nrequest_on_data += 1;

            slice = &slice[chunk.len()..];
            !slice.is_empty()
        });

        assert!(slice.is_empty());
        assert!(nrequest_on_data > 0);

        let buffer_length: u32 = data.len().try_into().unwrap();
        Box::leak(data);

        self.pending_buffers.insert(
            user_data,
            Pending::Write {
                nrequests: nrequest_on_data,
                buffer_length,
            },
        );
    }
}
