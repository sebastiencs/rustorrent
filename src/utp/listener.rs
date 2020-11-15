use std::{net::SocketAddr, sync::Arc};

use async_channel::{bounded, unbounded, Receiver, Sender, TrySendError};
use tokio::{net::ToSocketAddrs, sync::Mutex, task};
// use tokio::{net::UdpSocket, sync::RwLock, task};
use futures::future;
use hashbrown::HashMap;
use shared_arena::SharedArena;
use std::task::Context;
use tokio::{
    io::{Error, ErrorKind},
    sync::oneshot,
};

use super::udp_socket::MyUdpSocket as UdpSocket;

use super::{
    manager::{UtpEvent, UtpManager},
    stream::{State, UtpStream},
    tick::Tick,
    udp_socket::MmsgBuffer,
    Packet, PacketType, Result, Timestamp, UtpState, HEADER_SIZE, PACKET_MAX_SIZE,
};

const BUFFER_CAPACITY: usize = 1500;

struct Dispatcher {
    socket: Arc<UdpSocket>,
    sender: Sender<(UtpStream, SocketAddr)>,
    packet_arena: Arc<SharedArena<Packet>>,
    streams: HashMap<SocketAddr, Sender<UtpEvent>>,
    pending_connect: Arc<Mutex<HashMap<SocketAddr, Sender<UtpEvent>>>>,
    ticker: Sender<Sender<UtpEvent>>,
}

impl Dispatcher {
    fn new(
        socket: Arc<UdpSocket>,
        sender: Sender<(UtpStream, SocketAddr)>,
        packet_arena: Arc<SharedArena<Packet>>,
        pending_connect: Arc<Mutex<HashMap<SocketAddr, Sender<UtpEvent>>>>,
        ticker: Sender<Sender<UtpEvent>>,
    ) -> Self {
        Self {
            socket,
            sender,
            pending_connect,
            packet_arena,
            ticker,
            streams: HashMap::new(),
        }
    }

    async fn start(mut self) {
        Box::pin(self.process_incoming()).await.unwrap();
        println!("incoming done");
        panic!("DONE");
    }

    fn new_connection(
        socket: Arc<UdpSocket>,
        sockaddr: SocketAddr,
        packet_arena: Arc<SharedArena<Packet>>,
        state: impl Into<Option<State>>,
        on_connected: Option<oneshot::Sender<(UtpStream, SocketAddr)>>,
    ) -> Sender<UtpEvent> {
        // let socket = self.socket.clone();
        // let socket = self.get_matching_socket(&sockaddr);

        let (sender, receiver) = bounded(64);
        let state = state.into().unwrap_or_default();

        let manager = UtpManager::new_with_state(
            socket,
            sockaddr,
            receiver,
            sender.clone(),
            state,
            packet_arena,
            on_connected.into(),
        );

        task::spawn(async move {
            // Put state of the manager in a separate alloc
            // https://www.reddit.com/r/rust/comments/gz5oe0/futures_and_segmented_stacks
            Box::pin(manager.start()).await
        });

        sender
    }

    async fn poll_event(&self, buffers: &mut MmsgBuffer) -> Result<()> {
        let fun = |cx: &mut Context<'_>| self.socket.poll_recvmmsg(cx, buffers);

        future::poll_fn(fun).await.map_err(Into::into)
    }

    async fn process_incoming(&mut self) -> Result<()> {
        let mut buffers = MmsgBuffer::new();

        loop {
            self.poll_event(&mut buffers).await?;

            let timestamp = Timestamp::now();
            let mut buffers = buffers.iter_mmsgs();

            while let Some((addr, buffer)) = buffers.next() {
                if buffer.len() < HEADER_SIZE || buffer.len() > PACKET_MAX_SIZE {
                    continue;
                }

                let packet = self.packet_arena.alloc_with(|packet_uninit| {
                    Packet::from_incoming_in_place(packet_uninit, buffer, timestamp)
                });

                // println!("listener: data={:?}", packet);
                // println!("listener: data from={:?} length={:?} data={:?}", addr, buffer.len(), buffer);

                if let Some(addr) = self.streams.get(&addr) {
                    let incoming = UtpEvent::IncomingPacket { packet };

                    if let Err(TrySendError::Full(p)) = addr.try_send(incoming) {
                        let addr = addr.clone();
                        task::spawn(async move {
                            addr.send(p).await.unwrap();
                        });
                    }

                    continue;
                }

                match packet.get_type() {
                    Ok(PacketType::Syn) => {
                        let incoming = UtpEvent::IncomingPacket { packet };
                        let manager = Self::new_connection(
                            self.socket.clone(),
                            addr,
                            self.packet_arena.clone(),
                            None,
                            None,
                        );
                        self.ticker.try_send(manager.clone()).unwrap();
                        self.streams.insert(addr, manager.clone());
                        manager.send(incoming).await.unwrap();
                    }
                    Ok(PacketType::State) => {
                        if let Some(manager) = {
                            let mut pending = self.pending_connect.lock().await;
                            pending.remove(&addr)
                        } {
                            let incoming = UtpEvent::IncomingPacket { packet };
                            self.streams.insert(addr, manager.clone());
                            manager.send(incoming).await.unwrap();
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

pub struct UtpListener {
    socket: Arc<UdpSocket>,
    packet_arena: Arc<SharedArena<Packet>>,
    recv: Receiver<(UtpStream, SocketAddr)>,
    pending_connect: Arc<Mutex<HashMap<SocketAddr, Sender<UtpEvent>>>>,
    ticker: Sender<Sender<UtpEvent>>,
}

impl UtpListener {
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> UtpListener {
        let socket = Arc::new(UdpSocket::bind(addr).await.unwrap());
        let pending_connect = Arc::new(Mutex::new(HashMap::new()));
        let packet_arena = Arc::new(SharedArena::new());
        let (sender, recv) = bounded(8);
        let (ticker, tick_recv) = unbounded();

        task::spawn(async move { Tick::new(tick_recv).start().await });

        let ticker_clone = ticker.clone();
        let socket_clone = socket.clone();
        let pending_clone = pending_connect.clone();
        let arena_clone = packet_arena.clone();
        task::spawn(async move {
            Dispatcher::new(
                socket_clone,
                sender,
                arena_clone,
                pending_clone,
                ticker_clone,
            )
            .start()
            .await;
        });

        UtpListener {
            recv,
            packet_arena,
            pending_connect,
            socket,
            ticker,
        }
    }

    pub async fn accept(&self) -> (UtpStream, SocketAddr) {
        self.recv.recv().await.unwrap()
    }

    pub async fn connect(&self, sockaddr: SocketAddr) -> Result<UtpStream> {
        let (on_connected, is_connected) = oneshot::channel();

        let state = State::with_utp_state(UtpState::MustConnect);
        let manager = Dispatcher::new_connection(
            self.socket.clone(),
            sockaddr,
            self.packet_arena.clone(),
            state,
            on_connected.into(),
        );
        self.ticker.try_send(manager.clone()).unwrap();

        {
            let mut pending = self.pending_connect.lock().await;
            pending.insert(sockaddr, manager);
            drop(pending);
        }

        if let Ok((stream, _)) = is_connected.await {
            Ok(stream)
        } else {
            Err(Error::new(ErrorKind::TimedOut, "utp connect timed out").into())
        }
    }
}
