
use crossbeam_channel::{unbounded, Receiver, Sender};
use async_std::sync as a_sync;
use async_std::task;
use sha1::Sha1;

use std::sync::Arc;

use crate::pieces::PieceBuffer;
use crate::supervisors::torrent::TorrentNotification;

pub enum Sha1Task {
    CheckSum {
        /// Piece downloaded from a peer
        piece_buffer: Arc<PieceBuffer>,
        /// Sum in the metadata file
        sum_metadata: Arc<Vec<u8>>,
        id: usize,
        addr: a_sync::Sender<TorrentNotification>,
    }
}

use std::thread;

use packed_simd::u128x1;

#[allow(clippy::cast_ptr_alignment)]
#[inline(never)]
pub fn compare_20_bytes(sum1: &[u8], sum2: &[u8]) -> bool {
    if sum1.len() == 20 && sum2.len() == 20 {
        unsafe {
            let first1 = u128x1::from_slice_unaligned(&*(sum1 as *const [u8] as *const [u128]));
            let first2 = u128x1::from_slice_unaligned(&*(sum2 as *const [u8] as *const [u128]));

            let sum1: *const u32 = sum1.as_ptr().offset(16) as *const u32;
            let sum2: *const u32 = sum2.as_ptr().offset(16) as *const u32;

            first1 == first2 && *sum1 == *sum2
        }
    }
    else {
        panic!("Sums have 20 bytes")
    }
}


#[derive(Default, Debug)]
struct Sha1Worker {
    // valids: usize,
    // invalids: usize
}

impl Sha1Worker {
    fn start(mut self, recv: Receiver<Sha1Task>, task: impl Into<Option<Sha1Task>>) {
        if let Some(task) = task.into() {
            self.process(task);
        };

        while let Ok(task) = recv.recv() {
            self.process(task);
        }
    }

    fn process(&mut self, task: Sha1Task) {
        match task {
            Sha1Task::CheckSum { piece_buffer, sum_metadata, id, addr } => {
                let sha1 = crate::sha1::sha1(piece_buffer.buf.as_slice());

                let valid = compare_20_bytes(
                    &sha1[..],
                    sum_metadata.as_slice()
                );

                self.send_result(id, valid, addr);
            }
        }
    }

    fn send_result(&mut self, id: usize, valid: bool, addr: a_sync::Sender<TorrentNotification>) {
        use TorrentNotification::ResultChecksum;

        // if valid {
        //     self.valids += 1;
        // } else {
        //     self.invalids += 1;
        // }

        // println!("{:?}", self);

        task::spawn(async move {
            addr.send(ResultChecksum { id, valid }).await;
        });
    }
}

/// Thread workers are lazy started on the first task received
pub struct Sha1Workers;

impl Sha1Workers {
    pub fn new_pool() -> Sender<Sha1Task> {
        let (sender, receiver) = unbounded();

        thread::spawn(move || Self::start(receiver));

        sender
    }

    fn start(recv: Receiver<Sha1Task>) {
        if let Ok(first_task) = recv.recv() {
            let handles = Self::init_pool(first_task, recv);

            for handle in handles {
                let _ = handle.join();
            }
        }
    }

    fn init_pool(task: Sha1Task, receiver: Receiver<Sha1Task>) -> Vec<thread::JoinHandle<()>> {
        let num_cpus = num_cpus::get().max(1);
        let mut handles = Vec::with_capacity(num_cpus);

        let recv = receiver.clone();
        handles.push(thread::spawn(move || Sha1Worker::default().start(recv, task)));

        for _ in 0..(num_cpus - 1) {
            let recv = receiver.clone();
            handles.push(thread::spawn(move || Sha1Worker::default().start(recv, None)));
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
        assert_eq!(compare_sum(&vec1, &vec2), vec1 == vec2)
    }

    #[test]
    fn compare_sum_simd_slice() {
        let full = [1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1];
        assert_eq!(compare_sum(&full, &full), full == full);

        let slice = [1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0];
        assert_eq!(compare_sum(&slice, &full), slice == full);

        let slice = [0,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1];
        assert_eq!(compare_sum(&slice, &full), slice == full);

        let slice = [0,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0];
        assert_eq!(compare_sum(&slice, &full), slice == full);

        let slice = [1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0];
        assert_eq!(compare_sum(&full, &slice), full == slice);

        // test with the 17th byte
        let slice = [1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0,1,1,1];
        assert_eq!(compare_sum(&slice, &full), slice == full);
        assert_eq!(compare_sum(&slice, &slice), slice == slice);

        // test with the 16th byte
        let slice = [1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0,1,1,1,1];
        assert_eq!(compare_sum(&slice, &full), slice == full);
        assert_eq!(compare_sum(&slice, &slice), slice == slice);
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
