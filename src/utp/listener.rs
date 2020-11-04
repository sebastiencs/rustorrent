use std::{net::SocketAddr, sync::Arc, net::SocketAddrV4, net::SocketAddrV6, net::Ipv6Addr, net::Ipv4Addr};

use async_channel::{bounded, Sender};
use tokio::{net::UdpSocket, sync::RwLock, task};
use tokio::io::{ErrorKind, Error};
use hashbrown::HashMap;
use shared_arena::SharedArena;
use futures::{pin_mut, FutureExt, future};
use std::task::{Poll, Context};

use super::manager::{UtpEvent, UtpManager};
use super::{UtpState, Packet, Result, stream::UtpStream, tick::Tick, HEADER_SIZE, PACKET_MAX_SIZE, Timestamp, PacketType, stream::State};


const BUFFER_CAPACITY: usize = 1500;

pub struct UtpListener {
    v4: Arc<UdpSocket>,
    v6: Arc<UdpSocket>,
    /// The hashmap might be modified by different tasks so we wrap it in a RwLock
    streams: Arc<RwLock<HashMap<SocketAddr, Sender<UtpEvent>>>>,
    packet_arena: Arc<SharedArena<Packet>>
}

enum IncomingEvent {
    V4((usize, SocketAddr)),
    V6((usize, SocketAddr)),
}

impl UtpListener {
    pub async fn new(port: u16) -> Result<Arc<UtpListener>> {
        let v4 = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), port));
        let v6 = UdpSocket::bind(SocketAddrV6::new(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1), port, 0, 0));

        let (v4, v6) = futures::future::join(v4, v6).await;
        let (v4, v6) = (v4?, v6?);

        let listener = Arc::new(UtpListener {
            v4: Arc::new(v4),
            v6: Arc::new(v6),
            streams: Default::default(),
            packet_arena: Default::default()
        });

        listener.clone().start();

        Ok(listener)
    }

    fn get_matching_socket(&self, sockaddr: &SocketAddr) -> Arc<UdpSocket> {
        if sockaddr.is_ipv4() {
            Arc::clone(&self.v4)
        } else {
            Arc::clone(&self.v6)
        }
    }

    pub async fn connect(&self, sockaddr: SocketAddr) -> Result<UtpStream> {
        let (on_connected, is_connected) = bounded(1);

        let state = State::with_utp_state(UtpState::MustConnect);
        self.new_connection(sockaddr, state, on_connected).await;

        if let Ok(Some(stream)) = is_connected.recv().await {
            Ok(stream)
        } else {
            Err(Error::new(ErrorKind::TimedOut, "utp connect timed out").into())
        }
    }

    async fn new_connection(
        &self,
        sockaddr: SocketAddr,
        state: impl Into<Option<State>>,
        on_connected: impl Into<Option<Sender<Option<UtpStream>>>>,
    ) -> Sender<UtpEvent>
    {
        let socket = self.get_matching_socket(&sockaddr);

        let (sender, receiver) = bounded(10);
        let state = Arc::new(state.into().unwrap_or_else(Default::default));

        let manager = UtpManager::new_with_state(
            socket,
            sockaddr,
            receiver,
            state,
            self.packet_arena.clone(),
            on_connected.into(),
        );

        {
            let mut streams = self.streams.write().await;
            streams.insert(sockaddr, sender.clone());
        }

        task::spawn(async move {
            // Put state of the manager in a separate alloc
            // https://www.reddit.com/r/rust/comments/gz5oe0/futures_and_segmented_stacks
            Box::pin(manager.start()).await
        });

        sender
    }

    pub fn start(self: Arc<Self>) {
        task::spawn(async move {
            Tick::new(self.streams.clone()).start();
            Box::pin(self.process_incoming()).await.unwrap()
        });
    }

    async fn poll(&self, buffer_v4: &mut [u8], buffer_v6: &mut [u8]) -> Result<IncomingEvent> {
        let v4 = self.v4.recv_from(buffer_v4);
        let v6 = self.v6.recv_from(buffer_v6);
        pin_mut!(v4); // Pin on the stack
        pin_mut!(v6); // Pin on the stack

        let fun = |cx: &mut Context<'_>| {
            match FutureExt::poll_unpin(&mut v4, cx).map_ok(IncomingEvent::V4) {
                v @ Poll::Ready(_) => return v,
                _ => {}
            }

            match FutureExt::poll_unpin(&mut v6, cx).map_ok(IncomingEvent::V6) {
                v @ Poll::Ready(_) => v,
                _ => Poll::Pending
            }
        };

        future::poll_fn(fun)
            .await
            .map_err(Into::into)
    }

    async fn poll_event(&self, buffer_v4: &mut [u8], buffer_v6: &mut [u8]) -> Result<IncomingEvent> {
        loop {
            match self.poll(buffer_v4, buffer_v6).await {
                Err(ref e) if e.should_continue() => {
                    // WouldBlock or TimedOut
                    continue
                }
                x => return x
            }
        }
    }

    async fn process_incoming(&self) -> Result<()> {
        use IncomingEvent::*;

        let mut buffer_v4 = [0; BUFFER_CAPACITY];
        let mut buffer_v6 = [0; BUFFER_CAPACITY];

        loop {
            let (buffer, addr) = match self.poll_event(&mut buffer_v4[..], &mut buffer_v6[..]).await? {
                V4((size, addr)) => {
                    (&buffer_v4[..size], addr)
                },
                V6((size, addr)) => {
                    (&buffer_v6[..size], addr)
                },
            };

            if buffer.len() < HEADER_SIZE || buffer.len() > PACKET_MAX_SIZE {
                continue;
            }

            let timestamp = Timestamp::now();

            let packet = self.packet_arena.alloc_with(|packet_uninit| {
                Packet::from_incoming_in_place(packet_uninit, buffer, timestamp)
            });

            if let Some(addr) = {
                // Be sure to not borrow self.streams during
                // an await point
                self.streams.read().await.get(&addr).cloned()
            } {
                let incoming = UtpEvent::IncomingPacket { packet };
                addr.send(incoming).await;
                continue;
            };

            if let Ok(PacketType::Syn) = packet.get_type() {
                let incoming = UtpEvent::IncomingPacket { packet };
                self.new_connection(addr, None, None)
                    .await
                    .send(incoming)
                    .await;
            }
        }
    }
}
