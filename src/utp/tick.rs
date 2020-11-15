
use async_channel::{Sender, Receiver, TrySendError};
use tokio::sync::RwLock;
use std::net::SocketAddr;
use hashbrown::HashMap;

use std::sync::Arc;
use std::time::Duration;

use super::manager::UtpEvent;

pub struct Tick {
    streams: Vec<Sender<UtpEvent>>,
    recv: Receiver<Sender<UtpEvent>>
    // streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>,
}

impl Tick {
    pub fn new(recv: Receiver<Sender<UtpEvent>>) -> Self {
        Self { streams: vec![], recv }
    }

    pub async fn start(self) {
        self.main_loop().await;
    }

    async fn main_loop(mut self) {
        loop {
            while let Ok(sender) = self.recv.try_recv() {
                self.streams.push(sender);
            }

            tokio::time::sleep(Duration::from_millis(500)).await;

            self.streams.retain(|s| {
                match s.try_send(UtpEvent::Timeout) {
                    Err(TrySendError::Closed(_)) => false,
                    _ => true
                }
            });
        }
    }
}
