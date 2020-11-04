
use async_channel::Sender;
use tokio::sync::RwLock;
use std::net::SocketAddr;
use hashbrown::HashMap;

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::manager::UtpEvent;

pub struct Tick {
    streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>,
}

impl Tick {
    pub fn new(streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>) -> Self {
        Self { streams }
    }

    pub fn start(self) {
        tokio::spawn(async {
            self.main_loop().await;
        });
    }

    async fn main_loop(self) {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;

            let streams = Arc::clone(&self.streams);
            Self::send_tick(streams).await;
        }
    }

    async fn send_tick(streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>) {
        let streams = streams.read().await;
        for addr in streams.values() {
            // Tick it only when it's not too busy
            if !addr.is_full() {
                addr.send(UtpEvent::Tick).await;
            } else {
                println!("ADDR FULL !", );
            }
        }
    }

    // async fn send_tick(streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>) {
    //     let streams = streams.read().await;
    //     for addr in streams.values().filter(|p| !p.is_full()) {
    //         // Tick it only when it's not too busy
    //         addr.send(UtpEvent::Tick).await;
    //     }
    // }
}
