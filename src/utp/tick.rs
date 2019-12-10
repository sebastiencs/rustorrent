
use async_std::task;
use async_std::sync::{Sender, RwLock};
use async_std::net::SocketAddr;
use hashbrown::HashMap;

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::UtpEvent;

pub struct Tick {
    streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>,
}

impl Tick {
    pub fn new(streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>) -> Self {
        Self { streams }
    }

    pub fn start(self) {
        let builder = thread::Builder::new()
            .name("utp ticker".into());

        builder.spawn(move || {
            self.main_loop();
        }).unwrap();
    }

    fn main_loop(self) {
        loop {
            thread::sleep(Duration::from_millis(500));

            let streams = Arc::clone(&self.streams);
            task::spawn(async move {
                Self::send_tick(streams).await;
            });
        }
    }

    async fn send_tick(streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>) {
        let mut addrs_full = Vec::with_capacity(64);

        {
            let streams = streams.read().await;
            for addr in streams.values() {
                // TODO: Use try_send when available
                if !addr.is_full() {
                    addr.send(UtpEvent::Tick).await;
                } else {
                    addrs_full.push(addr.clone());
                }
            }
        }

        for addr in &addrs_full {
            addr.send(UtpEvent::Tick).await;
        }
    }
}
