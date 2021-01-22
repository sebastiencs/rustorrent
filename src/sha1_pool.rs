use async_channel::{Sender, TrySendError};
use crossbeam_channel::{unbounded, Receiver as SyncReceiver, Sender as SyncSender};
use tokio::runtime::Runtime;
use TorrentNotification::ValidatePiece;

use std::{ptr::read_unaligned, sync::Arc};

use crate::{
    fs::FSMessage,
    piece_picker::PieceIndex,
    torrent::{TorrentId, TorrentNotification},
};

pub enum Sha1Task {
    CheckSum {
        torrent_id: TorrentId,
        /// Piece downloaded from a peer
        piece: Box<[u8]>,
        /// Sum in the metadata file
        sum_metadata: Arc<[u8; 20]>,
        addr: Sender<TorrentNotification>,
        piece_index: PieceIndex,
    },
}

use std::thread;

#[allow(clippy::cast_ptr_alignment)]
#[inline(never)]
pub fn compare_20_bytes(sum1: &[u8], sum2: &[u8]) -> bool {
    assert!(sum1.len() == 20 && sum2.len() == 20, "Sums have 20 bytes");

    unsafe {
        let oword1 = read_unaligned(sum1.as_ptr().offset(0) as *const u128);
        let oword2 = read_unaligned(sum2.as_ptr().offset(0) as *const u128);

        let dword1 = read_unaligned(sum1.as_ptr().offset(16) as *const u32);
        let dword2 = read_unaligned(sum2.as_ptr().offset(16) as *const u32);

        oword1 == oword2 && dword1 == dword2
    }
}

#[derive(Debug)]
struct Sha1Worker {
    runtime: Arc<Runtime>,
    fs: Sender<FSMessage>,
}

impl Sha1Worker {
    fn new(runtime: Arc<Runtime>, fs: Sender<FSMessage>) -> Sha1Worker {
        Sha1Worker { runtime, fs }
    }

    fn start(mut self, recv: SyncReceiver<Sha1Task>, task: impl Into<Option<Sha1Task>>) {
        if let Some(task) = task.into() {
            self.process(task);
        };

        while let Ok(task) = recv.recv() {
            self.process(task);
        }
    }

    fn process(&mut self, task: Sha1Task) {
        match task {
            Sha1Task::CheckSum {
                piece,
                sum_metadata,
                addr,
                piece_index,
                torrent_id,
            } => {
                let sha1 = crate::sha1::sha1(&piece);

                let valid = compare_20_bytes(&sha1[..], &sum_metadata[..]);

                self.send_result(torrent_id, piece, valid, piece_index, addr);
            }
        }
    }

    fn send_result(
        &mut self,
        torrent_id: TorrentId,
        piece: Box<[u8]>,
        valid: bool,
        piece_index: PieceIndex,
        addr: Sender<TorrentNotification>,
    ) {
        let msg = ValidatePiece { piece_index, valid };
        if let Err(TrySendError::Full(msg)) = addr.try_send(msg) {
            tokio::spawn(async move { addr.send(msg).await });
        }

        if valid {
            let msg = FSMessage::Write {
                id: torrent_id,
                piece: piece_index,
                data: piece,
            };
            if let Err(TrySendError::Full(msg)) = self.fs.try_send(msg) {
                let fs = self.fs.clone();
                tokio::spawn(async move { fs.send(msg).await });
            }
        }
    }
}

/// Thread workers are lazy started on the first task received
pub struct Sha1Workers;

impl Sha1Workers {
    pub fn new_pool(runtime: Arc<Runtime>, fs: Sender<FSMessage>) -> SyncSender<Sha1Task> {
        let (sender, receiver) = unbounded();

        thread::spawn(move || Self::start(receiver, runtime, fs));

        sender
    }

    fn start(recv: SyncReceiver<Sha1Task>, runtime: Arc<Runtime>, fs: Sender<FSMessage>) {
        if let Ok(first_task) = recv.recv() {
            let handles = Self::init_pool(first_task, recv, runtime, fs);

            for handle in handles {
                let _ = handle.join();
            }
        }
    }

    fn init_pool(
        task: Sha1Task,
        receiver: SyncReceiver<Sha1Task>,
        runtime: Arc<Runtime>,
        fs: Sender<FSMessage>,
    ) -> Vec<thread::JoinHandle<()>> {
        let num_cpus = num_cpus::get().max(1).min(2);
        let mut handles = Vec::with_capacity(num_cpus);
        let mut task = Some(task);

        for index in 0..num_cpus {
            let recv = receiver.clone();
            let runtime_clone = runtime.clone();
            let fs_clone = fs.clone();
            let task = task.take();
            handles.push(
                thread::Builder::new()
                    .name(format!("sha-{}", index + 1))
                    .spawn(|| Sha1Worker::new(runtime_clone, fs_clone).start(recv, task))
                    .unwrap(),
            );
        }

        handles
    }
}

#[cfg(test)]
mod tests {
    use super::compare_20_bytes;

    #[test]
    fn compare_sum_simd() {
        let vec1 = vec![5; 20];
        let vec2 = vec1.clone();
        assert_eq!(compare_20_bytes(&vec1, &vec2), vec1 == vec2)
    }

    #[test]
    #[allow(clippy::eq_op)]
    fn compare_sum_simd_slice() {
        let full = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
        assert_eq!(compare_20_bytes(&full, &full), full == full);

        let slice = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0];
        assert_eq!(compare_20_bytes(&slice, &full), slice == full);

        let slice = [0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
        assert_eq!(compare_20_bytes(&slice, &full), slice == full);

        let slice = [0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0];
        assert_eq!(compare_20_bytes(&slice, &full), slice == full);

        let slice = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0];
        assert_eq!(compare_20_bytes(&full, &slice), full == slice);

        // test with the 17th byte
        let slice = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1];
        assert_eq!(compare_20_bytes(&slice, &full), slice == full);
        assert_eq!(compare_20_bytes(&slice, &slice), slice == slice);

        // test with the 16th byte
        let slice = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1];
        assert_eq!(compare_20_bytes(&slice, &full), slice == full);
        assert_eq!(compare_20_bytes(&slice, &slice), slice == slice);
    }

    #[test]
    #[should_panic]
    fn compare_sum_simd_short() {
        let vec1 = vec![5; 19];
        let vec2 = vec1.clone();
        compare_20_bytes(&vec1, &vec2);
    }

    #[test]
    #[should_panic]
    fn compare_sum_simd_different_size() {
        let vec1 = vec![5; 20];
        let mut vec2 = vec1.clone();
        vec2.push(1);
        compare_20_bytes(&vec1, &vec2);
    }

    #[test]
    #[should_panic]
    fn compare_sum_simd_different_size2() {
        let vec1 = vec![5; 19];
        let mut vec2 = vec1.clone();
        vec2.push(1);
        compare_20_bytes(&vec1, &vec2);
    }

    #[test]
    #[should_panic]
    fn compare_sum_simd_big() {
        let vec1 = vec![5; 21];
        let vec2 = vec1.clone();
        compare_20_bytes(&vec1, &vec2);
    }
}
