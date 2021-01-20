use std::{net::SocketAddr, sync::Arc};

use async_channel::{Receiver, Sender};
use futures::{future, StreamExt};
use hashbrown::HashMap;
// use kv_log_macro::error;
use tokio::{
    net::{TcpListener, TcpStream},
    runtime::Runtime,
};

use crate::{
    peer::stream::StreamBuffers,
    supervisors::torrent::{Result, TorrentNotification},
    utils::send_to,
};

// #[derive(Debug)]
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
                    // error!("New connection from {:?}", socket);
                    self.handle_new_peer(stream, socket).await.ok();
                }
                Ok((stream, socket)) = self.tcp6.accept() => {
                    // error!("New connection from {:?}", socket);
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

        let handshake = buffers.read_handshake().await.unwrap();

        if let Some(torrent) = self.torrents.get(&handshake.info_hash) {
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

#[cfg(test)]
mod tests {
    use std::{iter::repeat_with, sync::Arc, time::Duration};

    use async_channel::bounded;
    use tokio::runtime::Runtime;

    use crate::{
        peer::{message::MessagePeer, peer::PeerExternId, stream::StreamBuffers},
        supervisors::torrent::TorrentNotification,
    };

    use super::{Listener, ListenerMessage};

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support epoll
    fn listener() {
        let rt = Arc::new(Runtime::new().unwrap());
        let runtime = rt.clone();

        let hash: Vec<u8> = repeat_with(|| fastrand::u8(..)).take(20).collect();
        let hash_arc = hash.clone();

        println!("OK");

        rt.block_on(async move {
            let listener = Listener::new(runtime);

            let (sender, recv) = bounded(2);

            listener
                .send(ListenerMessage::AddTorrent {
                    sender,
                    info_hash: hash_arc.into_boxed_slice().into(),
                })
                .await
                .unwrap();

            let tcp = tokio::net::TcpStream::connect("localhost:6801")
                .await
                .unwrap();
            let mut stream = StreamBuffers::new(tcp, 1024, 1024);

            let id: Vec<u8> = repeat_with(|| fastrand::u8(..)).take(20).collect();

            let extern_id = PeerExternId::new(&id);

            stream
                .write_message(MessagePeer::Handshake {
                    info_hash: &hash,
                    extern_id: &extern_id,
                })
                .unwrap();

            tokio::time::sleep(Duration::from_millis(10)).await;

            match recv.try_recv().unwrap() {
                TorrentNotification::PeerAccepted { handshake, .. } => {
                    assert_eq!(&*handshake.info_hash, hash.as_slice());
                    assert_eq!(&handshake.extern_id, &extern_id);
                }
                _ => panic!(),
            }

            let tcp = tokio::net::TcpStream::connect("localhost:6801")
                .await
                .unwrap();
            let mut stream = StreamBuffers::new(tcp, 1024, 1024);

            let id: Vec<u8> = repeat_with(|| fastrand::u8(..)).take(20).collect();

            let extern_id = PeerExternId::new(&id);

            stream
                .write_message(MessagePeer::Handshake {
                    info_hash: &hash,
                    extern_id: &extern_id,
                })
                .unwrap();

            tokio::time::sleep(Duration::from_millis(10)).await;

            match recv.try_recv().unwrap() {
                TorrentNotification::PeerAccepted { handshake, .. } => {
                    assert_eq!(&*handshake.info_hash, hash.as_slice());
                    assert_eq!(&handshake.extern_id, &extern_id);
                }
                _ => panic!(),
            }

            let tcp = tokio::net::TcpStream::connect("[::1]:6801").await.unwrap();
            let mut stream = StreamBuffers::new(tcp, 1024, 1024);

            let id: Vec<u8> = repeat_with(|| fastrand::u8(..)).take(20).collect();

            let extern_id = PeerExternId::new(&id);

            stream
                .write_message(MessagePeer::Handshake {
                    info_hash: &hash,
                    extern_id: &extern_id,
                })
                .unwrap();

            tokio::time::sleep(Duration::from_millis(10)).await;

            match recv.try_recv().unwrap() {
                TorrentNotification::PeerAccepted { handshake, .. } => {
                    assert_eq!(&*handshake.info_hash, hash.as_slice());
                    assert_eq!(&handshake.extern_id, &extern_id);
                }
                _ => panic!(),
            }

            let msg = ListenerMessage::RemoveTorrent {
                info_hash: hash.into_boxed_slice().into(),
            };

            listener.send(msg).await.unwrap();
        });
    }
}
