use async_channel::{bounded, Sender};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use coarsetime::Instant;
use futures::{ready, StreamExt};
use kv_log_macro::{error, info};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};

use std::{
    convert::{TryFrom, TryInto},
    io::{Cursor, Write},
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use crate::{
    bitfield::BitFieldUpdate,
    errors::TorrentError,
    extensions::{ExtendedHandshake, ExtendedMessage, PEXMessage},
    piece_collector::Block,
    piece_picker::{BlockIndex, PieceIndex},
    pieces::{BlockToDownload, IterTaskDownload, Pieces, TaskDownload},
    spsc::{Consumer, Producer},
    supervisors::torrent::{NewPeer, Result, Shared, TorrentNotification},
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

#[derive(Debug)]
pub(crate) enum MessagePeer<'a> {
    KeepAlive,
    Choke,
    UnChoke,
    Interested,
    NotInterested,
    Have {
        piece_index: PieceIndex,
    },
    BitField(&'a [u8]),
    Request {
        index: PieceIndex,
        begin: BlockIndex,
        length: u32,
    },
    Piece {
        index: PieceIndex,
        begin: BlockIndex,
        block: &'a [u8],
    },
    Cancel {
        index: PieceIndex,
        begin: BlockIndex,
        length: u32,
    },
    Port(u16),
    Extension(ExtendedMessage<'a>),
    Unknown {
        id: u8,
        buffer: &'a [u8],
    },
}

impl<'a> TryFrom<&'a [u8]> for MessagePeer<'a> {
    type Error = TorrentError;

    fn try_from(buffer: &'a [u8]) -> Result<MessagePeer> {
        if buffer.is_empty() {
            return Ok(MessagePeer::KeepAlive);
        }
        let id = buffer[0];
        let buffer = &buffer[1..];
        Ok(match id {
            0 => MessagePeer::Choke,
            1 => MessagePeer::UnChoke,
            2 => MessagePeer::Interested,
            3 => MessagePeer::NotInterested,
            4 => {
                let mut cursor = Cursor::new(buffer);
                let piece_index = cursor.read_u32::<BigEndian>()?.into();

                MessagePeer::Have { piece_index }
            }
            5 => MessagePeer::BitField(buffer),
            6 => {
                let mut cursor = Cursor::new(buffer);
                let index = cursor.read_u32::<BigEndian>()?.into();
                let begin = cursor.read_u32::<BigEndian>()?.into();
                let length = cursor.read_u32::<BigEndian>()?;

                MessagePeer::Request {
                    index,
                    begin,
                    length,
                }
            }
            7 => {
                let mut cursor = Cursor::new(buffer);
                let index = cursor.read_u32::<BigEndian>()?.into();
                let begin = cursor.read_u32::<BigEndian>()?.into();
                let block = &buffer[8..];

                MessagePeer::Piece {
                    index,
                    begin,
                    block,
                }
            }
            8 => {
                let mut cursor = Cursor::new(buffer);
                let index = cursor.read_u32::<BigEndian>()?.into();
                let begin = cursor.read_u32::<BigEndian>()?.into();
                let length = cursor.read_u32::<BigEndian>()?;

                MessagePeer::Cancel {
                    index,
                    begin,
                    length,
                }
            }
            9 => {
                let mut cursor = Cursor::new(buffer);
                let port = cursor.read_u16::<BigEndian>()?;

                MessagePeer::Port(port)
            }
            20 => {
                let mut cursor = Cursor::new(buffer);
                let id = cursor.read_u8()?;

                match id {
                    0 => {
                        let handshake = crate::bencode::de::from_bytes(&buffer[1..])?;
                        MessagePeer::Extension(ExtendedMessage::Handshake {
                            handshake: Box::new(handshake),
                        })
                    }
                    _ => MessagePeer::Extension(ExtendedMessage::Message {
                        id,
                        buffer: &buffer[1..],
                    }),
                }
            }
            id => MessagePeer::Unknown { id, buffer },
        })
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

use hashbrown::HashMap;

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
    fn new(bytes: &[u8]) -> PeerExternId {
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

trait AsyncReadWrite: AsyncRead + AsyncWrite + Send {}
impl<T: AsyncRead + AsyncWrite + Send> AsyncReadWrite for T {}

struct PeerReadBuffer {
    reader: Pin<Box<dyn AsyncReadWrite>>,
    buffer: Box<[u8]>,
    pos: usize,
    msg_len: usize,
    pre_data: usize,
}

impl PeerReadBuffer {
    fn new<T>(stream: T, piece_length: usize) -> PeerReadBuffer
    where
        T: AsyncReadWrite + 'static,
    {
        PeerReadBuffer {
            reader: Box::pin(stream),
            buffer: vec![0; piece_length].into_boxed_slice(),
            pos: 0,
            msg_len: 0,
            pre_data: 0,
        }
    }

    fn get_mut(&mut self) -> Pin<&mut dyn AsyncReadWrite> {
        self.reader.as_mut()
    }

    fn buffer(&self) -> &[u8] {
        assert_ne!(self.msg_len, 0);
        &self.buffer[self.pre_data..self.msg_len]
    }

    fn consume(&mut self) {
        let pos = self.pos;
        let msg_len = self.msg_len;
        self.buffer.copy_within(msg_len..pos, 0);
        self.pos -= msg_len;
        self.msg_len = 0;
        self.pre_data = 0;
    }

    fn read_at_least(&mut self, n: usize, cx: &mut Context<'_>) -> Poll<Result<()>> {
        use tokio::io::ErrorKind::UnexpectedEof;

        while self.pos < n {
            let mut buf = ReadBuf::new(&mut self.buffer[self.pos..]);
            ready!(self.reader.as_mut().poll_read(cx, &mut buf))?;
            let filled = buf.filled().len();
            if filled == 0 {
                return Poll::Ready(Err(TorrentError::IO(UnexpectedEof.into())));
            }
            self.pos += filled;
        }

        Poll::Ready(Ok(()))
    }

    fn poll_handshake(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.read_at_least(1, cx))?;

        let length = self.buffer[0] as usize;

        ready!(self.read_at_least(length + 48, cx))?;

        self.pre_data = 1;
        self.msg_len = length + 48 + 1;
        Poll::Ready(Ok(()))
    }

    fn poll_next_message(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.read_at_least(4, cx))?;

        let length = {
            let mut cursor = Cursor::new(&self.buffer);
            cursor.read_u32::<BigEndian>().unwrap() as usize
        };

        assert!(length + 4 < self.buffer.len());

        ready!(self.read_at_least(length + 4, cx))?;

        self.pre_data = 4;
        self.msg_len = 4 + length;
        Poll::Ready(Ok(()))
    }

    async fn read_message(&mut self) -> Result<()> {
        futures::future::poll_fn(|cx| self.poll_next_message(cx)).await
    }

    async fn read_handshake(&mut self) -> Result<()> {
        futures::future::poll_fn(|cx| self.poll_handshake(cx)).await
    }
}

pub struct Peer {
    id: PeerId,
    addr: SocketAddr,
    supervisor: Sender<TorrentNotification>,
    reader: PeerReadBuffer,
    buffer: Vec<u8>,
    /// Are we choked from the peer
    choked: Choke,
    /// List of pieces to download
    tasks: Consumer<TaskDownload>,
    local_tasks: Option<IterTaskDownload>,

    pieces_infos: Arc<Pieces>,

    nblocks: usize,         // Downloaded
    start: Option<Instant>, // Downloaded,

    peer_detail: PeerDetail,

    extern_id: Arc<PeerExternId>,

    shared: Arc<Shared>,

    nrequested: usize,

    increase_requested: bool,
}

impl Peer {
    pub async fn new(
        addr: SocketAddr,
        pieces_infos: Arc<Pieces>,
        supervisor: Sender<TorrentNotification>,
        extern_id: Arc<PeerExternId>,
        consumer: Consumer<TaskDownload>,
    ) -> Result<Peer> {
        // TODO [2001:df0:a280:1001::3:1]:59632
        //      [2001:df0:a280:1001::3:1]:59632

        // if addr != "[2001:df0:a280:1001::3:1]:59632".parse::<SocketAddr>().unwrap() {
        //     return Err(TorrentError::InvalidInput);
        // }

        let stream = TcpStream::connect(&addr).await?;
        let piece_length = pieces_infos.piece_length;

        let id = PEER_COUNTER.fetch_add(1, Ordering::SeqCst);

        let shared = Arc::new(Shared::new());

        info!("[{}] Connected", id, { addr: addr.to_string(), piece_length: piece_length });

        Ok(Peer {
            addr,
            supervisor,
            pieces_infos,
            id: PeerId(id),
            extern_id,
            tasks: consumer,
            local_tasks: None,
            reader: PeerReadBuffer::new(stream, piece_length + 1024),
            buffer: Vec::with_capacity(32 * 1024),
            choked: Choke::Choked,
            nblocks: 0,
            start: None,
            peer_detail: Default::default(),
            shared,
            nrequested: 0,
            increase_requested: false,
        })
    }

    pub(crate) fn internal_id(&self) -> PeerId {
        self.id
    }

    async fn send_message<'m, M>(&mut self, msg: M) -> Result<()>
    where
        M: Into<MessagePeer<'m>>,
    {
        use tokio::io::AsyncWriteExt;

        self.write_message_in_buffer(msg.into());

        let mut writer = self.reader.get_mut();
        writer.write_all(self.buffer.as_slice()).await?;
        writer.flush().await?;

        Ok(())
    }

    fn write_message_in_buffer(&mut self, msg: MessagePeer<'_>) {
        self.buffer.clear();
        let mut cursor = Cursor::new(&mut self.buffer);

        match msg {
            MessagePeer::Choke => {
                cursor.write_u32::<BigEndian>(1).unwrap();
                cursor.write_u8(0).unwrap();
            }
            MessagePeer::UnChoke => {
                cursor.write_u32::<BigEndian>(1).unwrap();
                cursor.write_u8(1).unwrap();
            }
            MessagePeer::Interested => {
                cursor.write_u32::<BigEndian>(1).unwrap();
                cursor.write_u8(2).unwrap();
            }
            MessagePeer::NotInterested => {
                cursor.write_u32::<BigEndian>(1).unwrap();
                cursor.write_u8(3).unwrap();
            }
            MessagePeer::Have { piece_index } => {
                cursor.write_u32::<BigEndian>(5).unwrap();
                cursor.write_u8(4).unwrap();
                cursor.write_u32::<BigEndian>(piece_index.into()).unwrap();
            }
            MessagePeer::BitField(bitfield) => {
                cursor
                    .write_u32::<BigEndian>(1 + bitfield.len() as u32)
                    .unwrap();
                cursor.write_u8(5).unwrap();
                cursor.write_all(bitfield).unwrap();
            }
            MessagePeer::Request {
                index,
                begin,
                length,
            } => {
                cursor.write_u32::<BigEndian>(13).unwrap();
                cursor.write_u8(6).unwrap();
                cursor.write_u32::<BigEndian>(index.into()).unwrap();
                cursor.write_u32::<BigEndian>(begin.into()).unwrap();
                cursor.write_u32::<BigEndian>(length).unwrap();
            }
            MessagePeer::Piece {
                index,
                begin,
                block,
            } => {
                cursor
                    .write_u32::<BigEndian>(9 + block.len() as u32)
                    .unwrap();
                cursor.write_u8(7).unwrap();
                cursor.write_u32::<BigEndian>(index.into()).unwrap();
                cursor.write_u32::<BigEndian>(begin.into()).unwrap();
                cursor.write_all(block).unwrap();
            }
            MessagePeer::Cancel {
                index,
                begin,
                length,
            } => {
                cursor.write_u32::<BigEndian>(13).unwrap();
                cursor.write_u8(8).unwrap();
                cursor.write_u32::<BigEndian>(index.into()).unwrap();
                cursor.write_u32::<BigEndian>(begin.into()).unwrap();
                cursor.write_u32::<BigEndian>(length).unwrap();
            }
            MessagePeer::Port(port) => {
                cursor.write_u32::<BigEndian>(3).unwrap();
                cursor.write_u8(9).unwrap();
                cursor.write_u16::<BigEndian>(port).unwrap();
            }
            MessagePeer::KeepAlive => {
                cursor.write_u32::<BigEndian>(0).unwrap();
            }
            MessagePeer::Extension(ExtendedMessage::Handshake { handshake }) => {
                let bytes = crate::bencode::ser::to_bytes(&handshake).unwrap();
                cursor
                    .write_u32::<BigEndian>(2 + bytes.len() as u32)
                    .unwrap();
                cursor.write_u8(20).unwrap();
                cursor.write_u8(0).unwrap();
                cursor.write_all(&bytes).unwrap();
            }
            MessagePeer::Extension(ExtendedMessage::Message { .. }) => {}
            //MessagePeer::Extension { .. } => unreachable!()
            MessagePeer::Unknown { .. } => unreachable!(),
        }

        cursor.flush().unwrap();
    }

    fn writer(&mut self) -> Pin<&mut dyn AsyncReadWrite> {
        self.reader.get_mut()
    }

    pub async fn start(&mut self, producer: Producer<TaskDownload>) -> Result<()> {
        let (addr, cmds) = bounded(1000);
        let mut cmds = Box::pin(cmds);

        let extern_id = self.do_handshake().await?;

        self.supervisor
            .send(TorrentNotification::AddPeer {
                peer: Box::new(NewPeer {
                    id: self.id,
                    queue: producer,
                    addr,
                    socket: self.addr,
                    extern_id,
                    shared: Arc::clone(&self.shared),
                }),
            })
            .await
            .unwrap();

        loop {
            tokio::select! {
                msg = self.reader.read_message() => {
                    msg?;

                    self.dispatch().await?;
                    self.reader.consume();
                }
                cmd = cmds.next() => {
                    use PeerCommand::*;

                    match cmd {
                        Some(TasksAvailables) => {
                            self.maybe_request_block("task_available").await?;
                        }
                        Some(TasksIncreased) => {
                            self.increase_requested = false;
                        }
                        Some(Die) => {
                            return Ok(());
                        }
                        Some(BlockData { piece: _, block: _, data: _ }) => {
                            // send data to peer
                        }
                        None => {
                            // Disconnected
                        }
                    }
                }
            }
        }
    }

    fn pop_task(&mut self) -> Option<BlockToDownload> {
        loop {
            if let Some(task) = self.local_tasks.as_mut().and_then(|t| t.next()) {
                return Some(task);
            };

            let task = self.tasks.pop().ok()?;

            let local = task.iter_by_block(&self.pieces_infos);
            self.local_tasks.replace(local);
        }
    }

    async fn maybe_request_block(&mut self, caller: &'static str) -> Result<()> {
        if self.am_choked() {
            info!("[{}] Send interested", self.id);
            return self.send_message(MessagePeer::Interested).await;
        }

        // TODO [2001:df0:a280:1001::3:1]:59632

        // while self.nrequested < 5 {
        match self.pop_task() {
            Some(task) => {
                // info!("[{}] Task={:?}", self.id, task);
                self.send_message(task).await?;
            }
            _ => {
                if !self.increase_requested {
                    self.supervisor
                        .send(TorrentNotification::IncreaseTasksPeer { id: self.id })
                        .await
                        .unwrap();

                    self.increase_requested = true;
                }

                info!(
                    "[{}] No More Task ! {} downloaded in {:?}s caller={:?}",
                    self.id,
                    self.nblocks,
                    self.start.map(|s| s.elapsed().as_secs()),
                    caller,
                );
                // break;
            }
        }
        //     self.nrequested += 1;
        // }

        Ok(())
    }

    fn set_choked(&mut self, choked: bool) {
        self.choked = if choked {
            Choke::Choked
        } else {
            Choke::UnChoked
        };
    }

    fn am_choked(&self) -> bool {
        self.choked == Choke::Choked
    }

    async fn dispatch(&mut self) -> Result<()> {
        use MessagePeer::*;

        let msg = MessagePeer::try_from(self.reader.buffer())?;

        match msg {
            Choke => {
                self.set_choked(true);
                info!("[{}] Choke", self.id);
            }
            UnChoke => {
                // If the peer has piece we're interested in
                // Send a Request
                self.set_choked(false);
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
                use TorrentNotification::UpdateBitfield;

                let update = BitFieldUpdate::from(piece_index);

                self.supervisor
                    .send(UpdateBitfield {
                        id: self.id,
                        update: Box::new(update),
                    })
                    .await
                    .unwrap();

                info!("[{}] Have {:?}", self.id, piece_index);
            }
            BitField(bitfield) => {
                // Send an Interested ?
                use crate::bitfield::BitField;
                use TorrentNotification::UpdateBitfield;

                let bitfield = BitField::from(bitfield, self.pieces_infos.num_pieces)?;

                let update = BitFieldUpdate::from(bitfield);

                self.supervisor
                    .send(UpdateBitfield {
                        id: self.id,
                        update: Box::new(update),
                    })
                    .await
                    .unwrap();

                info!("[{}] Bitfield", self.id);
            }
            Request {
                index,
                begin,
                length,
            } => {
                // Mark this peer as interested
                // Make sure this peer is not choked or resend a choke
                info!("[{}] Request {:?} {:?} {}", self.id, index, begin, length);
            }
            Piece {
                index,
                begin,
                block,
            } => {
                // If we already have it, send another Request
                // Check the sum and write to disk
                // Send Request
                // info!("[{}] Block index={:?} begin={:?} length{}", self.id, index, begin, block.len());

                // self.nrequested -= 1;

                if self.start.is_none() {
                    self.start.replace(Instant::now());
                }

                self.nblocks += block.len();

                self.shared
                    .nbytes_on_tasks
                    .fetch_sub(block.len(), Ordering::Release);

                self.supervisor
                    .send(TorrentNotification::AddBlock {
                        id: self.id,
                        block: Block::from((index, begin, block)),
                    })
                    .await
                    .unwrap();

                self.maybe_request_block("block_received").await?;
            }
            Cancel {
                index,
                begin,
                length,
            } => {
                // Cancel a Request
                info!("[{}] Piece {:?} {:?} {}", self.id, index, begin, length);
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
                        self.supervisor
                            .send(TorrentNotification::PeerDiscovered {
                                addrs: addrs.into_boxed_slice(),
                            })
                            .await
                            .unwrap();
                    };
                }
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
        self.send_message(MessagePeer::Extension(ExtendedMessage::Handshake {
            handshake: Box::new(handshake),
        }))
        .await
    }

    async fn write(&mut self, data: &[u8]) -> Result<()> {
        let mut writer = self.writer();
        use tokio::prelude::*;

        writer.write_all(data).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn do_handshake(&mut self) -> Result<Arc<PeerExternId>> {
        let mut handshake: [u8; 68] = [0; 68];

        let mut cursor = Cursor::new(&mut handshake[..]);

        let mut reserved: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 0];

        reserved[5] |= 0x10; // Support Extension Protocol

        cursor.write_all(&[19])?;
        cursor.write_all(b"BitTorrent protocol")?;
        cursor.write_all(&reserved[..])?;
        cursor.write_all(self.pieces_infos.info_hash.as_ref())?;
        cursor.write_all(&**self.extern_id)?;

        self.write(&handshake).await?;

        self.reader.read_handshake().await?;
        let buffer = self.reader.buffer();
        let length = buffer.len();

        // TODO: Check the info hash and send to other TorrentSupervisor if necessary

        info!("[{}] Handshake done", self.id);

        let peer_id = PeerExternId::new(&buffer[length - 20..]);

        self.reader.consume();

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
