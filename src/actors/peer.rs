use async_channel::{bounded, Receiver, Sender};
use coarsetime::Instant;
use futures::StreamExt;
use kv_log_macro::{error, info};
use tokio::net::TcpStream;

use std::{
    convert::{TryFrom, TryInto},
    net::SocketAddr,
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
    supervisors::torrent::{NewPeer, Result, Shared, TorrentId, TorrentNotification},
    utils::send_to,
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
    TasksIncreased,
    BlockData {
        piece: PieceIndex,
        block: BlockIndex,
        data: Box<[u8]>,
    },
}

use hashbrown::{HashMap, HashSet};

#[derive(Default)]
struct PeerDetail {
    extension_ids: HashMap<String, i64>,
}

impl PeerDetail {
    fn update_with_extension(&mut self, ext: ExtendedHandshake) {
        if let Some(m) = ext.m {
            self.extension_ids = m;
        };
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
        super::sha1::compare_20_bytes(&self.0, &other.0)
    }
}

impl Eq for PeerExternId {}

#[derive(Hash, PartialEq, Eq)]
struct Requested {
    piece: PieceIndex,
    block: BlockIndex,
    length: u32,
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

    nblocks: usize,         // Downloaded
    start: Option<Instant>, // Downloaded,

    peer_detail: PeerDetail,

    extern_id: Arc<PeerExternId>,

    shared: Arc<Shared>,

    nrequested: usize,

    increase_requested: bool,

    requested_by_peer: HashSet<Requested>,
}

impl Peer {
    pub async fn new(
        torrent_id: TorrentId,
        socket: SocketAddr,
        pieces_infos: Arc<Pieces>,
        supervisor: Sender<TorrentNotification>,
        extern_id: Arc<PeerExternId>,
        consumer: Consumer<TaskDownload>,
        fs: Sender<FSMessage>,
    ) -> Result<Peer> {
        // TODO [2001:df0:a280:1001::3:1]:59632
        //      [2001:df0:a280:1001::3:1]:59632

        // if addr != "[2001:df0:a280:1001::3:1]:59632".parse::<SocketAddr>().unwrap() {
        //     return Err(TorrentError::InvalidInput);
        // }

        let stream = TcpStream::connect(&socket).await?;
        let piece_length = pieces_infos.piece_length;

        let id = PEER_COUNTER.fetch_add(1, Ordering::SeqCst);

        let shared = Arc::new(Shared::new(socket));

        info!("[{}] Connected", id, { addr: socket.to_string(), piece_length: piece_length });

        let (cmd_sender, cmd_recv) = bounded(1000);

        Ok(Peer {
            id: PeerId(id),
            cmd_sender,
            cmd_recv,
            torrent_id,
            supervisor,
            stream: StreamBuffers::new(stream, piece_length, 32 * 1024),
            choked: Choke::Choked,
            tasks: consumer,
            local_tasks: None,
            fs,
            pieces_infos,
            nblocks: 0,
            start: None,
            peer_detail: Default::default(),
            extern_id,
            shared,
            nrequested: 0,
            increase_requested: false,
            requested_by_peer: HashSet::default(),
        })
    }

    pub(crate) fn internal_id(&self) -> PeerId {
        self.id
    }

    pub async fn start(&mut self, producer: Producer<TaskDownload>) -> Result<()> {
        // let (addr, cmds) = bounded(1000);
        // let mut cmds = Box::pin(cmds);

        let extern_id = self.do_handshake().await?;

        send_to(
            &self.supervisor,
            TorrentNotification::AddPeer {
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
                msg = self.stream.read_message() => {
                    msg?;

                    self.dispatch().await?;
                    self.stream.consume_read();
                }
                cmd = recv.select_next_some() => {
                    use PeerCommand::*;

                    match cmd {
                        TasksAvailables => {
                            self.maybe_request_block("task_available").await?;
                        }
                        TasksIncreased => {
                            self.increase_requested = false;
                        }
                        Die => {
                            return Ok(());
                        }
                        BlockData { piece, block, data } => {
                            self.send_block(piece, block, data).await?;
                        }
                    }
                }
            }
        }
    }

    fn pop_task(&mut self) -> Option<BlockToDownload> {
        loop {
            if let Some(task) = self.local_tasks.as_mut().and_then(Iterator::next) {
                return Some(task);
            };

            let task = self.tasks.pop().ok()?;
            self.local_tasks
                .replace(task.iter_by_block(&self.pieces_infos));
        }
    }

    async fn maybe_request_block(&mut self, caller: &'static str) -> Result<()> {
        if self.am_choked() {
            info!("[{}] Send interested", self.id);
            return self.stream.write_message(MessagePeer::Interested).await;
        }

        // TODO [2001:df0:a280:1001::3:1]:59632

        // while self.nrequested < 5 {
        match self.pop_task() {
            Some(task) => {
                self.stream.write_message(task).await?;
                // self.send_message(task).await?;
            }
            _ => {
                if !self.increase_requested {
                    send_to(
                        &self.supervisor,
                        TorrentNotification::IncreaseTasksPeer { id: self.id },
                    );

                    self.increase_requested = true;

                    info!(
                        "[{}] No More Task ! {} downloaded in {:?}s caller={:?}",
                        self.id,
                        self.nblocks,
                        self.start.map(|s| s.elapsed().as_secs()),
                        caller,
                    );
                }
            }
        }
        //     self.nrequested += 1;
        // }

        Ok(())
    }

    async fn send_block(
        &mut self,
        piece: PieceIndex,
        block: BlockIndex,
        data: Box<[u8]>,
    ) -> Result<()> {
        let requested = Requested {
            piece,
            block,
            length: data.len().try_into().unwrap(),
        };

        if !self.requested_by_peer.contains(&requested) {
            // The peer canceled its request
            return Ok(());
        }

        self.stream
            .write_message(MessagePeer::Piece {
                piece,
                block,
                data: &data,
            })
            .await
    }

    fn am_choked(&self) -> bool {
        self.choked == Choke::Choked
    }

    async fn dispatch(&mut self) -> Result<()> {
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

                self.maybe_request_block("unchoke").await?;
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
                    TorrentNotification::UpdateBitfield {
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

                if let Ok(bitfield) = BitField::try_from((bitfield, num_pieces)) {
                    send_to(
                        &self.supervisor,
                        TorrentNotification::UpdateBitfield {
                            id: self.id,
                            update: Box::new(bitfield.into()),
                        },
                    );
                }

                info!("[{}] Bitfield", self.id);
            }
            Request {
                piece,
                block,
                length,
            } => {
                // Mark this peer as interested
                // Make sure this peer is not choked or resend a choke

                let requested = Requested {
                    piece,
                    block,
                    length,
                };

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
                // If we already have it, send another Request
                // Check the sum and write to disk
                // Send Request
                // info!("[{}] Block index={:?} begin={:?} length{}", self.id, index, begin, block.len());

                // self.nrequested -= 1;

                if self.start.is_none() {
                    self.start.replace(Instant::now());
                }

                self.nblocks += data.len();

                self.shared
                    .nbytes_on_tasks
                    .fetch_sub(data.len(), Ordering::Release);

                send_to(
                    &self.supervisor,
                    TorrentNotification::AddBlock {
                        id: self.id,
                        block: Block::from((piece, block, data)),
                    },
                );

                self.maybe_request_block("block_received").await?;
            }
            Cancel {
                piece,
                block,
                length,
            } => {
                let requested = Requested {
                    piece,
                    block,
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
            Extension(ExtendedMessage::Handshake { handshake: _ }) => {
                self.send_extended_handshake().await?;
                //self.maybe_send_request().await;
                //info!("[{}] EXTENDED HANDSHAKE SENT", self.id);
            }
            Extension(ExtendedMessage::Message { id, buffer }) => {
                if id == 1 {
                    if let Ok(addrs) = crate::bencode::de::from_bytes::<PEXMessage>(buffer) {
                        let addrs: Vec<SocketAddr> = addrs.into();
                        info!("[{}] new peers from pex {:?}", self.id, addrs);

                        send_to(
                            &self.supervisor,
                            TorrentNotification::PeerDiscovered {
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

    async fn send_extended_handshake(&mut self) -> Result<()> {
        let mut extensions = HashMap::new();
        extensions.insert("ut_pex".to_string(), 1);
        let handshake = ExtendedHandshake {
            m: Some(extensions),
            v: Some(String::from("Rustorrent 0.1")),
            p: Some(6801),
            ..Default::default()
        };
        self.stream
            .write_message(MessagePeer::Extension(ExtendedMessage::Handshake {
                handshake: Box::new(handshake),
            }))
            .await
    }

    async fn do_handshake(&mut self) -> Result<Arc<PeerExternId>> {
        self.stream
            .write_message(MessagePeer::Handshake {
                info_hash: &self.pieces_infos.info_hash,
                extern_id: &self.extern_id,
            })
            .await?;

        let peer_id = self.stream.read_handshake().await?;

        // TODO: Check the info hash and send to other TorrentSupervisor if necessary
        info!("[{}] Handshake done", self.id);

        Ok(Arc::new(peer_id))
    }
}

#[cfg(test)]
mod tests {
    use super::MessagePeer;

    fn assert_message_size() {
        assert_eq!(std::mem::size_of::<MessagePeer>(), 24);
    }
}
