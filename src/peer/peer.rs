use async_channel::{bounded, Receiver, Sender};
use byteorder::{BigEndian, ReadBytesExt};
use futures::StreamExt;
use kv_log_macro::{debug, error, info, warn};
use tokio::net::TcpStream;

use std::{
    convert::{TryFrom, TryInto},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use crate::{
    extensions::{ExtendedHandshake, ExtendedMessage, PEXMessage},
    fs::FSMessage,
    peer::{message::MessagePeer, stream::StreamBuffers},
    piece_collector::Block,
    piece_picker::{BlockIndex, PieceIndex},
    pieces::{BlockToDownload, IterTaskDownload, Pieces, TaskDownload},
    spsc::{Consumer, Producer},
    torrent::{
        NewPeer, Result, Shared, TorrentId,
        TorrentNotification::{self, *},
    },
    utils::{send_to, SaturatingDuration},
};

static PEER_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Eq, PartialEq, Copy, Clone, Debug, Hash)]
pub struct PeerId(usize);

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
impl PeerId {
    pub(crate) fn new(id: usize) -> Self {
        Self(id)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Choke {
    UnChoked,
    Choked,
}

#[derive(Debug)]
pub enum PeerCommand {
    TasksAvailables,
    Die,
    BlockData {
        piece: PieceIndex,
        block: BlockIndex,
        data: Box<[u8]>,
    },
}

use hashbrown::{HashMap, HashSet};

#[derive(Debug)]
struct PeerDetail {
    extension_ids: HashMap<String, i64>,
    // Number of requests the peer supports without dropping
    max_requests: Option<usize>,
    client_name: Option<String>,
    my_ip: Option<IpAddr>,
    ipv4: Option<Ipv4Addr>,
    ipv6: Option<Ipv6Addr>,
}

impl Default for PeerDetail {
    fn default() -> Self {
        Self {
            max_requests: None,
            extension_ids: HashMap::default(),
            client_name: None,
            my_ip: None,
            ipv4: None,
            ipv6: None,
        }
    }
}

/// Peer extern ID
/// Correspond to peer_id in the protocol and is 20 bytes long
pub struct PeerExternId([u8; 20]);

impl PeerExternId {
    pub fn new(bytes: &[u8]) -> PeerExternId {
        PeerExternId(bytes.try_into().expect("PeerExternId must be 20 bytes"))
    }

    pub fn generate() -> PeerExternId {
        use rand::{distributions::Alphanumeric, Rng};

        // TODO: Improve this

        const VERSION: usize = 1;

        let random = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(12)
            .collect::<String>();

        let s = format!("-RR{:04}-{}", VERSION, random);

        let id = s
            .as_bytes()
            .try_into()
            .expect("PeerExternId are 20 bytes long");

        PeerExternId(id)
    }
}

use std::ops::Deref;

use super::stream::HandshakeDetail;

impl Deref for PeerExternId {
    type Target = [u8; 20];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::fmt::Debug for PeerExternId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", String::from_utf8_lossy(&self.0))
    }
}

impl PartialEq for PeerExternId {
    fn eq(&self, other: &PeerExternId) -> bool {
        crate::sha1_pool::compare_20_bytes(&self.0, &other.0)
    }
}

impl Eq for PeerExternId {}

pub struct PeerInitialize {
    pub torrent_id: TorrentId,
    pub socket: SocketAddr,
    pub pieces_infos: Arc<Pieces>,
    pub supervisor: Sender<TorrentNotification>,
    pub extern_id: Arc<PeerExternId>,
    pub consumer: Consumer<TaskDownload>,
    pub fs: Sender<FSMessage>,
    pub buffers: Option<StreamBuffers>,
}

pub struct Peer {
    id: PeerId,
    torrent_id: TorrentId,

    cmd_sender: Sender<PeerCommand>,
    cmd_recv: Receiver<PeerCommand>,

    supervisor: Sender<TorrentNotification>,
    stream: StreamBuffers,
    /// Are we choked from the peer
    choked: Choke,
    /// List of pieces to download
    tasks: Consumer<TaskDownload>,
    local_tasks: Option<IterTaskDownload>,

    fs: Sender<FSMessage>,

    pieces_infos: Arc<Pieces>,

    peer_detail: PeerDetail,

    extern_id: Arc<PeerExternId>,

    shared: Arc<Shared>,

    requested_by_peer: HashSet<BlockToDownload>,
    requested_by_us: HashSet<BlockToDownload>,

    last_task_timestamp: Option<coarsetime::Instant>,
}

impl Peer {
    const MIN_REQUEST_IN_FLIGHT: usize = 10;
    const MAX_REQUEST_IN_FLIGHT_DEFAULT: usize = 250;

    pub async fn new(init: PeerInitialize) -> Result<Peer> {
        // TODO [2001:df0:a280:1001::3:1]:59632
        //      [2001:df0:a280:1001::3:1]:59632

        let id = PEER_COUNTER.fetch_add(1, Ordering::SeqCst);

        // if id > 0 {
        //     return Err(crate::errors::TorrentError::InvalidInput);
        // }

        // if socket == "[2001:df0:a280:1001::3:1]:59632".parse::<SocketAddr>().unwrap() {
        //     return Err(crate::errors::TorrentError::InvalidInput);
        // }

        // let socket = "[2001:df0:a280:1001::3:1]:59632".parse::<SocketAddr>().unwrap();

        let piece_length = init.pieces_infos.piece_length;

        let buffers = match init.buffers {
            Some(buffers) => buffers,
            None => {
                let stream = TcpStream::connect(&init.socket).await?;
                StreamBuffers::new(stream, piece_length, 32 * 1024)
            }
        };

        let shared = Arc::new(Shared::new(init.socket));

        info!("[{}] Connected", id, { addr: init.socket.to_string(), piece_length: piece_length });

        let (cmd_sender, cmd_recv) = bounded(1000);

        Ok(Peer {
            id: PeerId(id),
            cmd_sender,
            cmd_recv,
            torrent_id: init.torrent_id,
            supervisor: init.supervisor,
            stream: buffers,
            choked: Choke::Choked,
            tasks: init.consumer,
            local_tasks: None,
            fs: init.fs,
            pieces_infos: init.pieces_infos,
            peer_detail: Default::default(),
            extern_id: init.extern_id,
            shared,
            requested_by_peer: HashSet::default(),
            requested_by_us: HashSet::default(),
            last_task_timestamp: None,
        })
    }

    pub(crate) fn internal_id(&self) -> PeerId {
        self.id
    }

    pub async fn start(
        &mut self,
        producer: Producer<TaskDownload>,
        handshake: Option<Box<HandshakeDetail>>,
    ) -> Result<()> {
        // let (addr, cmds) = bounded(1000);
        // let mut cmds = Box::pin(cmds);

        let extern_id = self.do_handshake(handshake).await?;

        send_to(
            &self.supervisor,
            AddPeer {
                peer: Box::new(NewPeer {
                    id: self.id,
                    queue: producer,
                    addr: self.cmd_sender.clone(),
                    extern_id,
                    shared: Arc::clone(&self.shared),
                }),
            },
        );

        let mut recv = self.cmd_recv.clone().fuse();

        loop {
            tokio::select! {
                msg = self.stream.wait_on_socket() => {
                    msg?;

                    self.dispatch()?;
                    self.stream.consume_read();
                }
                cmd = recv.select_next_some() => {
                    use PeerCommand::*;

                    match cmd {
                        TasksAvailables => {
                            self.handle_new_tasks()?;
                        }
                        Die => {
                            return Ok(());
                        }
                        BlockData { piece, block, data } => {
                            self.send_block(piece, block, data)?;
                        }
                    }
                }
            }
        }
    }

    fn handle_new_tasks(&mut self) -> Result<()> {
        let now = coarsetime::Instant::now();

        let increase_requests = self
            .last_task_timestamp
            .map(|i| now.saturating_duration_since(i).as_millis() < 3000)
            .unwrap_or(true);

        self.last_task_timestamp.replace(now);

        if increase_requests || self.requested_by_us.is_empty() {
            self.maybe_request_block("task_available")?;
        }

        Ok(())
    }

    fn pop_task(&mut self) -> Option<BlockToDownload> {
        let max_requests = self
            .peer_detail
            .max_requests
            .unwrap_or(Self::MAX_REQUEST_IN_FLIGHT_DEFAULT);

        if self.requested_by_us.len() >= max_requests {
            return None;
        }

        loop {
            if let Some(task) = self.local_tasks.as_mut().and_then(Iterator::next) {
                return Some(task);
            };

            let task = self.tasks.pop().ok()?;
            self.local_tasks
                .replace(task.iter_by_block(&self.pieces_infos));
        }
    }

    fn is_empty_task(&self) -> bool {
        let local_empty = self
            .local_tasks
            .as_ref()
            .map(IterTaskDownload::is_empty)
            .unwrap_or(true);
        local_empty && self.tasks.is_empty()
    }

    fn maybe_request_block(&mut self, _caller: &'static str) -> Result<()> {
        if self.am_choked() {
            info!("[{}] Send interested", self.id);
            return Ok(self.stream.write_message(MessagePeer::Interested)?);
        }

        while let Some(task) = self.pop_task() {
            self.stream.write_message(task.clone())?;
            self.requested_by_us.insert(task);

            if self.is_empty_task() {
                send_to(&self.supervisor, IncreaseTasksPeer { id: self.id });
                break;
            }

            if self.requested_by_us.len() >= Self::MIN_REQUEST_IN_FLIGHT {
                break;
            }
        }

        // TODO [2001:df0:a280:1001::3:1]:59632

        Ok(())
    }

    fn send_block(&mut self, piece: PieceIndex, block: BlockIndex, data: Box<[u8]>) -> Result<()> {
        let requested = BlockToDownload {
            piece,
            start: block,
            length: data.len().try_into().unwrap(),
        };

        if !self.requested_by_peer.contains(&requested) {
            // The peer canceled its request
            return Ok(());
        }

        self.stream.write_message(MessagePeer::Piece {
            piece,
            block,
            data: &data,
        })?;

        Ok(())
    }

    fn am_choked(&self) -> bool {
        self.choked == Choke::Choked
    }

    fn dispatch(&mut self) -> Result<()> {
        use MessagePeer::*;

        let msg = self.stream.get_message()?;

        match msg {
            Choke => {
                self.choked = self::Choke::Choked;
                info!("[{}] Choke", self.id);
            }
            UnChoke => {
                // If the peer has piece we're interested in
                // Send a Request
                self.choked = self::Choke::UnChoked;

                info!("[{}] Unchoke", self.id);

                self.maybe_request_block("unchoke")?;
            }
            Interested => {
                // Unshoke this peer
                info!("[{}] Interested", self.id);
            }
            NotInterested => {
                // Shoke this peer
                info!("[{}] Not interested", self.id);
            }
            Have { piece_index } => {
                send_to(
                    &self.supervisor,
                    UpdateBitfield {
                        id: self.id,
                        update: Box::new(piece_index.into()),
                    },
                );

                info!("[{}] Have {:?}", self.id, piece_index);
            }
            BitField(bitfield) => {
                // Send an Interested ?
                use crate::bitfield::BitField;

                let num_pieces = self.pieces_infos.num_pieces;

                let bitfield = match BitField::try_from((bitfield, num_pieces)) {
                    Ok(bitfield) => bitfield,
                    _ => return Ok(()),
                };

                send_to(
                    &self.supervisor,
                    UpdateBitfield {
                        id: self.id,
                        update: Box::new(bitfield.into()),
                    },
                );

                info!("[{}] Bitfield", self.id);
            }
            Request {
                piece,
                block,
                length,
            } => {
                // Mark this peer as interested
                // Make sure this peer is not choked or resend a choke

                let requested = BlockToDownload {
                    piece,
                    start: block,
                    length,
                };

                info!("[{}] Requested by Peer {:?}", self.id, requested);

                if self.requested_by_peer.contains(&requested) {
                    return Ok(());
                }

                send_to(
                    &self.fs,
                    FSMessage::Read {
                        id: self.torrent_id,
                        piece,
                        block,
                        length,
                        peer: self.cmd_sender.clone(),
                    },
                );

                self.requested_by_peer.insert(requested);

                info!("[{}] Request {:?} {:?} {}", self.id, piece, block, length);
            }
            Piece { piece, block, data } => {
                // info!("[{}] Receive Block index={:?} begin={:?} length{}", self.id, piece, block, data.len());

                let recv = BlockToDownload {
                    piece,
                    start: block,
                    length: data.len().try_into().unwrap(),
                };

                if !self.requested_by_us.remove(&recv) {
                    warn!("[{}] Received but not requested {:?}", self.id, recv);
                }

                self.shared
                    .nbytes_on_tasks
                    .fetch_sub(data.len(), Ordering::Release);

                send_to(
                    &self.supervisor,
                    AddBlock {
                        id: self.id,
                        block: Block::from((piece, block, data)),
                    },
                );

                self.maybe_request_block("block_received")?;
            }
            Cancel {
                piece,
                block,
                length,
            } => {
                let requested = BlockToDownload {
                    piece,
                    start: block,
                    length,
                };

                self.requested_by_peer.remove(&requested);

                // Cancel a Request
                info!("[{}] Piece {:?} {:?} {}", self.id, piece, block, length);
            }
            Port(port) => {
                info!("[{}] Port {}", self.id, port);
            }
            KeepAlive => {
                info!("[{}] Keep alive", self.id);
            }
            Extension(ExtendedMessage::Handshake { handshake }) => {
                self.read_extended_handshake(&handshake);
                self.send_extended_handshake()?;
            }
            Extension(ExtendedMessage::Message { id, buffer }) => {
                if id == 1 {
                    if let Ok(addrs) = crate::bencode::de::from_bytes::<PEXMessage>(buffer) {
                        let addrs: Vec<SocketAddr> = addrs.into();
                        info!("[{}] new peers from pex {:?}", self.id, addrs);

                        send_to(
                            &self.supervisor,
                            PeerDiscovered {
                                addrs: addrs.into_boxed_slice(),
                            },
                        );
                    };
                }
            }
            Handshake { .. } => {
                // If we read a handshake here, it means the peer sent more than
                // one handshake. Ignore
            }
            Unknown { id, buffer } => {
                // Check extension
                // Disconnect
                error!(
                    "[{}] Unknown {:?} {}",
                    self.id,
                    id,
                    String::from_utf8_lossy(buffer)
                );
            }
        }
        Ok(())
    }

    fn read_extended_handshake(&mut self, handshake: &ExtendedHandshake) {
        self.peer_detail.max_requests = handshake.reqq.and_then(|m| m.try_into().ok());
        self.peer_detail.extension_ids = handshake.m.as_ref().cloned().unwrap_or_default();
        self.peer_detail.client_name = handshake.v.clone();

        self.peer_detail.my_ip = handshake
            .yourip
            .as_ref()
            .map(|ip| {
                ip.as_slice()
                    .read_u128::<BigEndian>()
                    .map(|ip| Ipv6Addr::from(ip).into())
                    .or_else(|_| {
                        ip.as_slice()
                            .read_u32::<BigEndian>()
                            .map(|ip| Ipv4Addr::from(ip).into())
                    })
                    .ok()
            })
            .flatten();

        self.peer_detail.ipv4 = handshake
            .ipv4
            .as_ref()
            .map(|ip| {
                ip.as_slice()
                    .read_u32::<BigEndian>()
                    .map(Ipv4Addr::from)
                    .ok()
            })
            .flatten();

        self.peer_detail.ipv6 = handshake
            .ipv6
            .as_ref()
            .map(|ip| {
                ip.as_slice()
                    .read_u128::<BigEndian>()
                    .map(Ipv6Addr::from)
                    .ok()
            })
            .flatten();

        // error!("[{}] {:#?}", self.id, handshake);
        // error!("[{}] {:#?}", self.id, self.peer_detail);
    }

    fn read_extended_message(&self, id: u8, buffer: &[u8]) {
        match id {
            1 => {
                let addrs = match crate::bencode::de::from_bytes::<PEXMessage>(buffer) {
                    Ok(addrs) => addrs,
                    Err(_) => return,
                };

                let addrs: Vec<SocketAddr> = addrs.into();
                info!("[{}] new peers from pex {:?}", self.id, addrs);

                send_to(
                    &self.supervisor,
                    PeerDiscovered {
                        addrs: addrs.into_boxed_slice(),
                    },
                );
            }
            id => {
                debug!("[{}] Unsupported extended message {}", self.id, id);
            }
        }
    }

    fn send_extended_handshake(&mut self) -> Result<()> {
        let mut extensions = HashMap::new();
        extensions.insert("ut_pex".to_string(), 1);
        let handshake = ExtendedHandshake {
            m: Some(extensions),
            v: Some(String::from("Rustorrent 0.1")),
            p: Some(6801),
            ..Default::default()
        };
        self.stream.write_message(handshake)?;

        Ok(())
    }

    async fn do_handshake(
        &mut self,
        handshake: Option<Box<HandshakeDetail>>,
    ) -> Result<Arc<PeerExternId>> {
        self.stream.write_message(MessagePeer::Handshake {
            info_hash: &self.pieces_infos.info_hash,
            extern_id: &self.extern_id,
        })?;

        let handshake_detail = match handshake {
            Some(handshake) => *handshake,
            _ => self.stream.read_handshake().await?,
        };

        // TODO: Check the info hash and send to other TorrentSupervisor if necessary
        info!("[{}] Handshake done", self.id);

        Ok(Arc::new(handshake_detail.extern_id))
    }
}

#[cfg(test)]
mod tests {
    use super::MessagePeer;

    fn assert_message_size() {
        assert_eq!(std::mem::size_of::<MessagePeer>(), 24);
    }
}
