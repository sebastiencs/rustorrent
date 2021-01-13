use std::{net::SocketAddr, sync::Arc};

use async_channel::{Receiver, Sender};
use futures::{future, StreamExt};
use hashbrown::HashMap;
use kv_log_macro::error;
use tokio::{
    net::{TcpListener, TcpStream},
    runtime::Runtime,
};

use crate::{
    peer::stream::StreamBuffers,
    supervisors::torrent::{Result, TorrentNotification},
    utils::send_to,
};

#[derive(Debug)]
pub enum ListenerMessage {
    AddTorrent {
        sender: Sender<TorrentNotification>,
        info_hash: Arc<[u8]>,
    },
    RemoveTorrent {
        info_hash: Arc<[u8]>,
    },
}

pub struct Listener {
    tcp: TcpListener,
    tcp6: TcpListener,
    torrents: HashMap<Arc<[u8]>, Sender<TorrentNotification>>,
}

impl Listener {
    #[allow(clippy::clippy::new_ret_no_self)]
    pub fn new(runtime: Arc<Runtime>) -> Sender<ListenerMessage> {
        let (sender, recv) = async_channel::bounded(32);

        runtime.spawn(Box::pin(async move {
            let tcp = TcpListener::bind("0.0.0.0:6801");
            let tcp6 = TcpListener::bind("::1:6801");

            let (tcp, tcp6) = future::join(tcp, tcp6).await;

            Self {
                tcp: tcp.unwrap(),
                tcp6: tcp6.unwrap(),
                torrents: HashMap::default(),
            }
            .start(recv)
            .await;
        }));

        sender
    }

    pub async fn start(mut self, recv: Receiver<ListenerMessage>) {
        let mut recv = recv.fuse();

        loop {
            tokio::select! {
                Ok((stream, socket)) = self.tcp.accept() => {
                    error!("New connection from {:?}", socket);
                    self.handle_new_peer(stream, socket).await.ok();
                }
                Ok((stream, socket)) = self.tcp6.accept() => {
                    error!("New connection from {:?}", socket);
                    self.handle_new_peer(stream, socket).await.ok();
                }
                msg = recv.next() => {
                    let msg = match msg {
                        Some(msg) => msg,
                        _ => return
                    };

                    self.handle_msg(msg);
                }
            }
        }
    }

    async fn handle_new_peer(&mut self, stream: TcpStream, socket: SocketAddr) -> Result<()> {
        let mut buffers = StreamBuffers::new(stream, 512, 0);

        let handshake = buffers.read_handshake().await?;

        if let Some(torrent) = self.torrents.get(&handshake.info_hash) {
            error!("Found torrent {:?}", handshake.info_hash);
            send_to(
                torrent,
                TorrentNotification::PeerAccepted {
                    buffers,
                    socket,
                    handshake: Box::new(handshake),
                },
            );
        }

        Ok(())
    }

    fn handle_msg(&mut self, msg: ListenerMessage) {
        match msg {
            ListenerMessage::AddTorrent { sender, info_hash } => {
                self.torrents.insert(info_hash, sender);
            }
            ListenerMessage::RemoveTorrent { info_hash } => {
                self.torrents.remove(&info_hash);
            }
        }
    }
}
