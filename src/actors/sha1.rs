
use crossbeam_channel::{unbounded, Receiver, Sender};
use async_std::sync as a_sync;
use async_std::task;
use sha1::Sha1;

use std::sync::Arc;

use crate::pieces::PieceBuffer;
use crate::supervisors::torrent::TorrentNotification;

pub enum Sha1Task {
    CheckSum {
        piece_buffer: Arc<PieceBuffer>,
        sum2: Arc<Vec<u8>>,
        id: usize,
        addr: a_sync::Sender<TorrentNotification>,
    }
}

use std::thread;

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
            Sha1Task::CheckSum { piece_buffer, sum2, id, addr } => {
                let sha1 = Sha1::from(piece_buffer.buf.as_slice()).digest();
                let sha1 = sha1.bytes();

                let valid = &sha1[..] == sum2.as_slice();

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
