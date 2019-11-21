
use crossbeam_channel::{unbounded, Receiver, Sender};

pub struct ToCheck {

}

use std::thread;

struct Sha1Worker;

impl Sha1Worker {
    fn start(recv: Receiver<ToCheck>, task: impl Into<Option<ToCheck>>) {
        if let Some(task) = task.into() {
            Self::process(task);
        };

        while let Ok(task) = recv.recv() {
            Self::process(task);
        }
    }

    fn process(task: ToCheck) {

    }
}

/// Thread workers are lazy started on the first task received
pub struct Sha1Workers;

impl Sha1Workers {
    pub fn new_pool() -> Sender<ToCheck> {
        let (sender, receiver) = unbounded();

        thread::spawn(move || Self::start(receiver));

        sender
    }

    fn start(recv: Receiver<ToCheck>) {
        if let Ok(first_task) = recv.recv() {
            let handles = Self::init_pool(first_task, recv);

            for handle in handles {
                let _ = handle.join();
            }
        }
    }

    fn init_pool(task: ToCheck, receiver: Receiver<ToCheck>) -> Vec<thread::JoinHandle<()>> {
        let num_cpus = num_cpus::get().max(1);
        let mut handles = Vec::with_capacity(num_cpus);

        let recv = receiver.clone();
        handles.push(thread::spawn(move || Sha1Worker::start(recv, task)));

        for _ in 0..(num_cpus - 1) {
            let recv = receiver.clone();
            handles.push(thread::spawn(move || Sha1Worker::start(recv, None)));
        }

        handles
    }
}
