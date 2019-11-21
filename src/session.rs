use crate::metadata::Torrent;
use crate::http_client::{self, AnnounceQuery, AnnounceResponse};
use crate::bitfield::BitField;
use crate::utils::{FromSlice, Map};

use crate::http_client::{Peers,Peers6};
use std::io::Cursor;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

fn get_peers_addrs(response: &AnnounceResponse) -> Vec<SocketAddr> {
    let mut addrs = Vec::new();

    match response.peers6 {
        Some(Peers6::Binary(ref bin)) => {
            addrs.reserve(bin.len() / 18);

            for chunk in bin.chunks_exact(18) {
                let mut cursor = Cursor::new(chunk);
                let mut addr: [u16; 9] = [0; 9];
                for i in 0..9 {
                    match cursor.read_u16::<BigEndian>() {
                        Ok(value) => addr[i] = value,
                        _ => continue
                    }
                }
                let ipv6 = Ipv6Addr::new(addr[0], addr[1], addr[2], addr[3], addr[4], addr[5], addr[6], addr[7]);
                let ipv6 = SocketAddrV6::new(ipv6, addr[8], 0, 0);

                addrs.push(ipv6.into());
            }
        }
        Some(Peers6::Dict(ref peers)) => {
            addrs.reserve(peers.len());

            for peer in peers {
                match (peer.ip.as_str(), peer.port).to_socket_addrs() {
                    Ok(socks) => {
                        for addr in socks {
                            addrs.push(addr);
                        }
                    }
                    Err(e) => {
                        // TODO: report to the user
                    }
                }
            }
        }
        _ => {}
    }

    match response.peers {
        Some(Peers::Binary(ref bin)) => {
            addrs.reserve(bin.len() / 6);

            for chunk in bin.chunks_exact(6) {
                let mut cursor = Cursor::new(&chunk[..]);

                let ipv4 = match cursor.read_u32::<BigEndian>() {
                    Ok(ipv4) => ipv4,
                    _ => continue
                };

                let port = match cursor.read_u16::<BigEndian>() {
                    Ok(port) => port,
                    _ => continue
                };

                let ipv4 = SocketAddrV4::new(Ipv4Addr::from(ipv4), port);

                addrs.push(ipv4.into());
            }
        }
        Some(Peers::Dict(ref peers)) => {
            addrs.reserve(peers.len());

            for peer in peers {
                match (peer.ip.as_str(), peer.port).to_socket_addrs() {
                    Ok(socks) => {
                        for addr in socks {
                            addrs.push(addr);
                        }
                    }
                    Err(e) => {
                        // TODO: report to the user
                    }
                }
            }
        }
        _ => {}
    }

    println!("ADDRS: {:#?}", addrs);
    addrs
}

use std::io::prelude::*;
use smallvec::{SmallVec, smallvec};

use url::Url;

use crate::http_client::HttpError;
use crossbeam_channel::{unbounded, Receiver, Sender};

#[derive(Debug)]
pub enum TorrentError {
    InvalidInput,
    Http(HttpError),
    IO(std::io::Error),
    IOAsync(async_std::io::Error)
}

impl From<HttpError> for TorrentError {
    fn from(e: HttpError) -> TorrentError {
        match e {
            HttpError::IO(e) => TorrentError::IO(e),
            HttpError::IOAsync(e) => TorrentError::IOAsync(e),
            e => TorrentError::Http(e)
        }
    }
}

impl From<async_std::io::Error> for TorrentError {
    fn from(e: async_std::io::Error) -> TorrentError {
        TorrentError::IOAsync(e)
    }
}

#[derive(Debug)]
struct Tracker {
    url: Url,
    announce: Option<AnnounceResponse>
}

impl Tracker {
    fn new(url: Url) -> Tracker {
        Tracker { url, announce: None }
    }

    fn announce(&mut self, torrent: &Torrent) -> Result<Vec<SocketAddr>> {
        let query = AnnounceQuery::from(torrent);
        let response = http_client::get(&self.url, query)?;

        let peers = get_peers_addrs(&response);
        self.announce = Some(response);

        Ok(peers)
    }
}

// enum PeerState {
//     Connecting,
//     Handshaking,
//     Downloading {
//         piece: usize,
//         index: usize,
//     },
//     Dead
// }

#[derive(Debug, PartialEq, Eq)]
enum Choke {
    UnChoked,
    Choked
}

use std::collections::VecDeque;

type PeerTask = Arc<async_std::sync::RwLock<VecDeque<PieceToDownload>>>;

#[derive(Debug)]
struct PieceInfo {
    bytes_downloaded: usize,
    workers: SmallVec<[PeerTask; 4]>
}

impl PieceInfo {
    fn new(queue: PeerTask) -> PieceInfo {
        PieceInfo {
            bytes_downloaded: 0,
            workers: smallvec![queue]
        }
    }

    // fn new(peer: &Peer) -> PieceInfo {
    //     PieceInfo {
    //         bytes_downloaded: 0,
    //         workers: smallvec![peer.tasks.clone()]
    //     }
    // }
}

// enum PieceActorMessage {
//     AddQueue {
//         id: PeerId,
//         queue: PeerTask
//     },
//     RemoveQueue {
//         id: PeerId ,
//         queue: PeerTask
//     },
//     AddPiece(PieceBuffer)
//     // AddPiece {
//     //     index: u32,
//     //     begin: u32,
//     //     block: Vec<u8>
//     // }
// }

// #[derive(Debug)]
// struct PiecesActor {
//     data: TorrentData,
//     pieces: async_std::sync::RwLock<Vec<Option<PieceInfo>>>,
//     nblocks_piece: usize,
//     block_size: usize,
//     files_size: usize,
//     piece_length: usize,
//     last_piece_size: usize,
//     npieces: usize,
//     queue_map: async_std::sync::RwLock<HashMap<PeerId, PeerTask>>,
//     channel: async_std::sync::Receiver<PieceActorMessage>
// }

type PeerId = usize;

use sha1::Sha1;

// impl PiecesActor {
//     fn new(data: &TorrentData, channel: async_std::sync::Receiver<PieceActorMessage>) -> PiecesActor {
//         let (npieces, nblocks_piece, block_size, files_size, piece_length, last_piece_size) = data.with(|data| {
//             let piece_length = data.pieces.piece_length;
//             let total = data.torrent.files_total_size();
//             let last_piece_size = total % piece_length;

//             (data.pieces.num_pieces,
//              data.pieces.nblocks_piece,
//              data.pieces.block_size,
//              total,
//              piece_length,
//              if last_piece_size == 0 { piece_length } else { last_piece_size }
//             )
//         });

//         let mut p = Vec::with_capacity(npieces);
//         for _ in 0..npieces {
//             p.push(None);
//         }

//         PiecesActor {
//             data: data.clone(),
//             nblocks_piece,
//             block_size,
//             channel,
//             files_size,
//             piece_length,
//             last_piece_size,
//             npieces,
//             pieces: async_std::sync::RwLock::new(p),
//             queue_map: async_std::sync::RwLock::new(HashMap::new()),
//         }
//     }

//     async fn set_task_queue(&self, id: PeerId, queue: PeerTask) {
//         let mut queue_map = self.queue_map.write().await;
//         queue_map.insert(id, queue);
//     }

//     fn piece_size(&self, piece_index: usize) -> usize {
//         if piece_index == self.npieces - 1 {
//             self.last_piece_size
//         } else {
//             self.piece_length
//         }
//     }

//     async fn start(&self) {
//         use PieceActorMessage::*;

//         while let Some(msg) = self.channel.recv().await {
//             match msg {
//                 RemoveQueue { id, queue } => {
//                     {
//                         let mut queue_map = self.queue_map.write().await;
//                         queue_map.remove(&id);
//                     }
//                     {
//                         let mut pieces = self.pieces.write().await;
//                         for piece in pieces.iter_mut().filter_map(Option::as_mut) {
//                             piece.workers.retain(|p| {
//                                 !Arc::ptr_eq(&p, &queue)
//                             });
//                         }
//                     }
//                 }
//                 AddQueue { id, queue } => {

//                 }
//                 AddPiece (piece_block) => {

//                     let sha1_torrent = self.data.with(|data| {
//                         data.pieces.sha1_pieces.get(piece_block.piece_index).map(Arc::clone)
//                     });

//                     if let Some(sha1_torrent) = sha1_torrent {
//                         let sha1 = Sha1::from(&piece_block.buf).digest();
//                         let sha1 = sha1.bytes();
//                         if sha1 == sha1_torrent.as_slice() {
//                             //println!("SHA1 ARE GOOD !! {}", piece_block.piece_index);
//                         } else {
//                             println!("WRONG SHA1 :() {}", piece_block.piece_index);
//                         }
//                     } else {
//                         println!("PIECE RECEIVED BUT NOT FOUND {}", piece_block.piece_index);
//                     }


// //                    println!("PIECE RECEIVED {} {}", piece_block.piece_index, piece_block.buf.len());
//                 }
//             }
//         }
//     }

//     async fn get_pieces_to_downloads(&self, peer: &Peer, update: &BitFieldUpdate) {
//         let mut pieces = self.pieces.write().await;
//         let queue_map = self.queue_map.read().await;
//         let mut queue = match queue_map.get(&peer.id) {
//             Some(queue) => queue.write().await,
//             _ => return
//         };

//         match update {
//             BitFieldUpdate::BitField(bitfield) => {
//                 let pieces = pieces.iter_mut()
//                                    .enumerate()
//                                    .filter(|(index, p)| p.is_none() && bitfield.get_bit(*index))
//                                    .take(5);

//                 let nblock_piece = self.nblocks_piece;
//                 let block_size = self.block_size;

//                 let mut i = 0;
//                 for (piece, value) in pieces {
//                     for i in 0..nblock_piece {
//                         queue.push_back(PieceToDownload::new(piece, i * block_size, block_size));
//                     }
//                     //println!("[{:?}] PUSHING PIECE={}", peer.id, piece);
//                     value.replace(PieceInfo::new(peer));
//                     i += 1;
//                 }
//             }
//             BitFieldUpdate::Piece(piece) => {
//                 let piece = *piece;

//                 if piece >= pieces.len() {
//                     return;
//                 }

//                 if pieces.get(piece).unwrap().is_none() {
//                     let nblock_piece = self.nblocks_piece;
//                     let block_size = self.block_size;

//                     for i in 0..nblock_piece {
//                         queue.push_back(PieceToDownload::new(piece, i * block_size, block_size));
//                     }

//                     //println!("[{:?}] _PUSHING PIECE={}", peer.id, piece);
//                     pieces.get_mut(piece).unwrap().replace(PieceInfo::new(peer));
//                 }

//             }
//         }
//     }
// }

#[derive(Clone, Debug)]
struct PieceToDownload {
    piece: u32,
    start: u32,
    size: u32
}

impl PieceToDownload {
    fn new(piece: usize, start: usize, size: usize) -> PieceToDownload {
        PieceToDownload { piece: piece as u32, start: start as u32, size: size as u32 }
    }
}

use coarsetime::{Duration, Instant};

struct Peer {
    id: PeerId,
    addr: SocketAddr,
    data: TorrentData,
    supervisor: a_sync::Sender<PeerMessage>,
    //pieces_actor: Arc<PiecesActor>,
    //pieces_actor_chan: async_std::sync::Sender<PieceActorMessage>,
    reader: BufReader<TcpStream>,
    //state: PeerState,
    buffer: Vec<u8>,
    /// Are we choked from the peer
    choked: Choke,
    /// BitField of the peer
    bitfield: BitField,
    /// List of pieces to download
    tasks: PeerTask,

    /// Small buffer where the downloaded block are kept.
    /// Once the piece is full, we send this buffer to PieceActor
    /// and reset this vec.
    piece_buffer: Map<usize, PieceBuffer>,

    pieces_detail: Pieces,

    nblocks: usize, // Downloaded
    start: Option<Instant>, // Downloaded,
    _tasks: Option<VecDeque<PieceToDownload>>,
    npieces: usize,
}

use async_std::sync::Mutex;

use async_std::net::TcpStream;
use async_std::prelude::*;
use async_std::io::{BufReader, BufWriter};

#[derive(Debug)]
enum MessagePeer<'a> {
    KeepAlive,
    Choke,
    UnChoke,
    Interested,
    NotInterested,
    Have {
        piece_index: u32
    },
    BitField(&'a [u8]),
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        block: &'a [u8],
    },
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    Port(u16),
    Unknown(u8)
}

use std::convert::TryFrom;

impl<'a> TryFrom<&'a [u8]> for MessagePeer<'a> {
    type Error = TorrentError;

    fn try_from(buffer: &'a [u8]) -> Result<MessagePeer> {
        let id = buffer[0];
        let buffer = &buffer[1..];
        Ok(match id {
            0 => MessagePeer::Choke,
            1 => MessagePeer::UnChoke,
            2 => MessagePeer::Interested,
            3 => MessagePeer::NotInterested,
            4 => {
                let mut cursor = Cursor::new(buffer);
                let piece_index = cursor.read_u32::<BigEndian>()?;

                MessagePeer::Have { piece_index }
            }
            5 => {
                MessagePeer::BitField(buffer)
            }
            6 => {
                let mut cursor = Cursor::new(buffer);
                let index = cursor.read_u32::<BigEndian>()?;
                let begin = cursor.read_u32::<BigEndian>()?;
                let length = cursor.read_u32::<BigEndian>()?;

                MessagePeer::Request { index, begin, length }
            }
            7 => {
                let mut cursor = Cursor::new(buffer);
                let index = cursor.read_u32::<BigEndian>()?;
                let begin = cursor.read_u32::<BigEndian>()?;
                let block = &buffer[8..];

                MessagePeer::Piece { index, begin, block }
            }
            8 => {
                let mut cursor = Cursor::new(buffer);
                let index = cursor.read_u32::<BigEndian>()?;
                let begin = cursor.read_u32::<BigEndian>()?;
                let length = cursor.read_u32::<BigEndian>()?;

                MessagePeer::Cancel { index, begin, length }
            }
            9 => {
                let mut cursor = Cursor::new(buffer);
                let port = cursor.read_u16::<BigEndian>()?;

                MessagePeer::Port(port)
            }
            x => MessagePeer::Unknown(x)
        })
    }
}

use std::borrow::Cow;

pub enum BitFieldUpdate {
    BitField(BitField),
    Piece(usize)
}

impl From<u32> for BitFieldUpdate {
    fn from(p: u32) -> BitFieldUpdate {
        BitFieldUpdate::Piece(p as usize)
    }
}

impl From<BitField> for BitFieldUpdate {
    fn from(b: BitField) -> BitFieldUpdate {
        BitFieldUpdate::BitField(b)
    }
}

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

struct PieceBuffer {
    buf: Vec<u8>,
    piece_index: usize,
    bytes_added: usize,
}

impl PieceBuffer {
    /// piece_size might be different than piece_length if it's the last piece
    fn new(piece_index: usize, piece_size: usize) -> PieceBuffer {
        let mut buf = Vec::with_capacity(piece_size);
        unsafe { buf.set_len(piece_size) };

        // println!("ADDING PIECE_BUFFER {} SIZE={}", piece_index, piece_size);

        PieceBuffer { buf, piece_index, bytes_added: 0 }
    }

    fn new_with_block(
        piece_index: u32,
        piece_size: usize,
        begin: u32,
        block: &[u8]
    ) -> PieceBuffer {
        let mut piece_buffer = PieceBuffer::new(piece_index as usize, piece_size);
        piece_buffer.add_block(begin, block);
        piece_buffer
    }

    /// begin: offset of the block in the piece
    fn add_block(&mut self, begin: u32, block: &[u8]) {
        let block_len = block.len();

        let begin = begin as usize;
        let buffer_len = self.buf.len();
        if let Some(buffer) = self.buf.get_mut(begin..begin + block_len) {
            buffer.copy_from_slice(block);
            self.bytes_added += block_len;
        } else {
            panic!("ERROR on addblock BEGIN={} BLOCK_LEN={} BUF_LEN={}", begin, block_len, buffer_len);
        }
    }

    fn added(&self) -> usize {
        self.bytes_added
    }

    /// The piece is considered completed once added block size match with
    /// the piece size. There is no check where the blocks have been added
    fn is_completed(&self) -> bool {
        self.buf.len() == self.bytes_added
    }
}

static PEER_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
enum PeerCmd {
    TasksAvailables
}

use futures::future::{Fuse, FutureExt};
use std::pin::Pin;

enum PeerWaitEvent {
    Msg(Result<usize>),
    Cmd(Option<PeerCmd>),
}

impl Peer {
    async fn new(
        addr: SocketAddr,
        data: TorrentData,
        pieces_detail: Pieces,
        supervisor: a_sync::Sender<PeerMessage>,
    ) -> Result<Peer> {
        let stream = TcpStream::connect(&addr).await?;

        let id = PEER_COUNTER.fetch_add(1, Ordering::SeqCst);

        let bitfield = BitField::new(pieces_detail.num_pieces);

        Ok(Peer {
            addr,
            supervisor,
            npieces: pieces_detail.num_pieces,
            pieces_detail,
            data,
            bitfield,
            id,
            tasks: PeerTask::default(),
            reader: BufReader::with_capacity(32 * 1024, stream),
            buffer: Vec::with_capacity(32 * 1024),
            choked: Choke::Choked,
            nblocks: 0,
            start: None,
            _tasks: None,
            piece_buffer: Map::default(),
        })
    }

    async fn send_message(&mut self, msg: MessagePeer<'_>) -> Result<()> {
        self.write_message_in_buffer(msg);

        let writer = self.reader.get_mut();
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
                cursor.write_u32::<BigEndian>(piece_index).unwrap();
            }
            MessagePeer::BitField (bitfield) => {
                cursor.write_u32::<BigEndian>(1 + bitfield.len() as u32).unwrap();
                cursor.write_u8(5).unwrap();
                cursor.write_all(bitfield).unwrap();
            }
            MessagePeer::Request { index, begin, length } => {
                cursor.write_u32::<BigEndian>(13).unwrap();
                cursor.write_u8(6).unwrap();
                cursor.write_u32::<BigEndian>(index).unwrap();
                cursor.write_u32::<BigEndian>(begin).unwrap();
                cursor.write_u32::<BigEndian>(length).unwrap();
            }
            MessagePeer::Piece { index, begin, block } => {
                cursor.write_u32::<BigEndian>(9 + block.len() as u32).unwrap();
                cursor.write_u8(7).unwrap();
                cursor.write_u32::<BigEndian>(index).unwrap();
                cursor.write_u32::<BigEndian>(begin).unwrap();
                cursor.write_all(block).unwrap();
            }
            MessagePeer::Cancel { index, begin, length } => {
                cursor.write_u32::<BigEndian>(13).unwrap();
                cursor.write_u8(8).unwrap();
                cursor.write_u32::<BigEndian>(index).unwrap();
                cursor.write_u32::<BigEndian>(begin).unwrap();
                cursor.write_u32::<BigEndian>(length).unwrap();
            }
            MessagePeer::Port (port) => {
                cursor.write_u32::<BigEndian>(3).unwrap();
                cursor.write_u8(9).unwrap();
                cursor.write_u16::<BigEndian>(port).unwrap();
            }
            MessagePeer::KeepAlive => {
                cursor.write_u32::<BigEndian>(0).unwrap();
            }
            MessagePeer::Unknown (_) => unreachable!()
        }

        cursor.flush();
    }

    fn writer(&mut self) -> &mut TcpStream {
        self.reader.get_mut()
    }

    async fn wait_event(
        &mut self,
        mut cmds: Pin<&mut Fuse<impl Future<Output = Option<PeerCmd>>>>
    ) -> PeerWaitEvent {
        use futures::async_await::*;
        use futures::task::{Context, Poll};
        use futures::future;
        use pin_utils::pin_mut;

        let mut msgs = Box::pin(self.read_messages());
        pin_mut!(msgs);

        assert_unpin(&msgs);
        assert_unpin(&cmds);
        //assert_fused_future();

        let mut fun = |cx: &mut Context<'_>| {
            match FutureExt::poll_unpin(&mut msgs, cx).map(PeerWaitEvent::Msg) {
                v @ Poll::Ready(_) => return v,
                _ => {}
            }

            match FutureExt::poll_unpin(&mut cmds, cx).map(PeerWaitEvent::Cmd) {
                v @ Poll::Ready(_) => return v,
                _ => Poll::Pending
            }
        };

        future::poll_fn(fun).await
    }

    async fn start(&mut self) -> Result<()> {

        let (addr, cmds) = a_sync::channel(1000);

        self.supervisor.send(PeerMessage::AddPeer {
            id: self.id,
            queue: self.tasks.clone(),
            addr,
        }).await;

        self.do_handshake().await?;

        let cmds = cmds.recv().fuse();
        let mut cmds = Box::pin(cmds);

        loop {
            match self.wait_event(cmds.as_mut()).await {
                PeerWaitEvent::Msg(Ok(n)) => {
                    if n != 0 {
                        self.dispatch().await?;
                    } // else Keep Alive
                }
                PeerWaitEvent::Cmd(..) => {
                    self.maybe_send_request().await;
                }
                PeerWaitEvent::Msg(Err(e)) => {
                    eprintln!("[{}] PEER ERROR MSG {:?}", self.id, e);
                    return Err(e);
                }
            }
        }

        Ok(())
    }

    async fn take_tasks(&mut self) -> Option<PieceToDownload> {
        if self._tasks.is_none() {
            let t = self.tasks.read().await;
            self._tasks = Some(t.clone());
        }
        self._tasks.as_mut().and_then(|mut t| t.pop_front())
    }

    async fn maybe_send_request(&mut self) -> Result<()> {
        if !self.am_choked() {

            let task = match self._tasks.as_mut() {
                Some(mut tasks) => tasks.pop_front(),
                _ => self.take_tasks().await
            };

            if let Some(task) = task {
                self.send_request(task).await?;
            } else {
                //self.pieces_actor.get_pieces_to_downloads().await;
                println!("[{:?}] No More Task ! {} downloaded in {:?}s", self.id, self.nblocks, self.start.map(|s| s.elapsed().as_secs()));
                // Steal others tasks
            }
        } else {
            self.send_message(MessagePeer::Interested).await;
            println!("[{}] SENT INTERESTED", self.id);
        }
        Ok(())
    }

    async fn send_request(&mut self, task: PieceToDownload) -> Result<()> {
        self.send_message(MessagePeer::Request {
            index: task.piece,
            begin: task.start,
            length: task.size,
        }).await
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

    async fn dispatch<'a>(&'a mut self) -> Result<()> {
        use MessagePeer::*;

        let msg = MessagePeer::try_from(self.buffer.as_slice())?;

        match msg {
            Choke => {
                self.set_choked(true);
                println!("[{}] CHOKE", self.id);
            },
            UnChoke => {
                // If the peer has piece we're interested in
                // Send a Request
                self.set_choked(false);
                println!("[{}] UNCHOKE", self.id);

                self.maybe_send_request().await?;
            },
            Interested => {
                // Unshoke this peer
                println!("INTERESTED", );
            },
            NotInterested => {
                // Shoke this peer
                println!("NOT INTERESTED", );
            },
            Have { piece_index } => {
                let update = BitFieldUpdate::from(piece_index);

                self.supervisor.send(PeerMessage::UpdateBitfield { id: self.id, update }).await;

                println!("[{:?}] HAVE {}", self.id, piece_index);

                //self.cmds.recv().await;

                //self.pieces_actor.get_pieces_to_downloads(&self, &update).await;

                // self.update_bitfield(update);

                // if self.am_choked() {
                //     self.send_message(MessagePeer::Interested).await?;
                // } else {
                //     self.maybe_send_request().await?;
                // }
            },
            BitField (bitfield) => {
                // Send an Interested ?

                let bitfield = crate::bitfield::BitField::from(
                    bitfield,
                    self.npieces
                    //self.data.with(|data| data.pieces.num_pieces)
                )?;

                let update = BitFieldUpdate::from(bitfield);

                self.supervisor.send(PeerMessage::UpdateBitfield { id: self.id, update }).await;

                //self.cmds.recv().await;

                println!("[{:?}] BITFIELD", self.id);

                //self.pieces_actor.get_pieces_to_downloads(&self, &update).await;

                // self.update_bitfield(update);

                // if self.am_choked() {
                //     self.send_message(MessagePeer::Interested).await?;
                // } else {
                //     self.maybe_send_request().await?;
                // }
            },
            Request { index, begin, length } => {
                // Mark this peer as interested
                // Make sure this peer is not choked or resend a choke
                println!("REQUEST {} {} {}", index, begin, length);
            },
            Piece { index, begin, block } => {
                // If we already have it, send another Request
                // Check the sum and write to disk
                // Send Request
                //println!("[{:?}] PIECE {} {} {}", self.id, index, begin, block.len());

                if self.start.is_none() {
                    self.start.replace(Instant::now());
                }

                self.nblocks += block.len();

                let piece_size = self.pieces_detail.piece_size(index as usize);
                let mut is_completed = false;

                self.piece_buffer
                    .entry(index as usize)
                    .and_modify(|p| { p.add_block(begin, block); is_completed = p.is_completed() })
                    .or_insert_with(|| PieceBuffer::new_with_block(index, piece_size, begin, block));

                if is_completed {
                    self.send_completed(index).await;
                }

                self.maybe_send_request().await?;
            },
            Cancel { index, begin, length } => {
                // Cancel a Request
                println!("PIECE {} {} {}", index, begin, length);
            },
            Port (port) => {
                println!("PORT {}", port);
            },
            KeepAlive => {
                println!("KEEP ALICE");
            }
            Unknown (x) => {
                // Check extension
                // Disconnect
                println!("UNKNOWN {:?}", x);
            }
        }
        Ok(())
    }

    async fn send_completed(&mut self, index: u32) {
        let piece_buffer = self.piece_buffer.remove(&(index as usize)).unwrap();

        self.supervisor.send(PeerMessage::AddPiece(piece_buffer)).await;
        //println!("[{}] PIECE COMPLETED {}", self.id, index);
    }

    async fn read_messages(&mut self) -> Result<usize> {
        self.read_exactly(4).await?;

        let length = {
            let mut cursor = Cursor::new(&self.buffer);
            cursor.read_u32::<BigEndian>()? as usize
        };

        if length == 0 {
            return Ok(0); // Keep Alive
        }

        self.read_exactly(length).await?;

        Ok(length)
    }

    async fn read_exactly(&mut self, n: usize) -> Result<()> {
        let reader = self.reader.by_ref();
        self.buffer.clear();

        if reader.take(n as u64).read_to_end(&mut self.buffer).await? != n {
            return Err(async_std::io::Error::new(
                async_std::io::ErrorKind::UnexpectedEof,
                "Size doesn't match"
            ).into());
        }

        Ok(())
    }

    async fn write(&mut self, data: &[u8]) -> Result<()> {
        let writer = self.writer();
        writer.write_all(data).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn do_handshake(&mut self) -> Result<()> {
        let mut handshake: [u8; 68] = [0; 68];

        let mut cursor = Cursor::new(&mut handshake[..]);

        cursor.write(&[19])?;
        cursor.write(b"BitTorrent protocol")?;
        cursor.write(&[0,0,0,0,0,0,0,0])?;
        self.data.with(|data| {
            cursor.write(data.torrent.info_hash.as_ref());
        });
        cursor.write(b"-RT1220sJ1Nna5rzWLd8")?;

        self.write(&handshake).await?;

        self.read_exactly(1).await?;
        let len = self.buffer[0] as usize;
        self.read_exactly(len + 48).await?;

        // TODO: Check the info hash and send to other TorrentSupervisor if necessary

        println!("HANDSHAKE DONE", );

        Ok(())
    }

}

// enum MessageActor {
//     AddPeer(PeerAddr),
//     RemovePeer(PeerAddr),
// }

#[derive(Debug, Clone)]
struct Pieces {
    /// Number of pieces
    num_pieces: usize,
    /// SHA1 of each piece
    sha1_pieces: Arc<Vec<Arc<Vec<u8>>>>,
    /// Pieces other peers have
    /// peers_pieces[0] is the number of peers having the piece 0
//    peers_pieces: Vec<u8>,
    /// Size of a block
    block_size: usize,
    /// Number of block in 1 piece
    nblocks_piece: usize,
    /// Number of block in the last piece
    nblocks_last_piece: usize,
    /// Piece length
    piece_length: usize,
    /// Last piece length
    last_piece_length: usize,
    /// Total files size
    files_size: usize,
}

impl From<&Torrent> for Pieces {
    fn from(torrent: &Torrent) -> Pieces {
        let sha1_pieces = Arc::new(torrent.sha_pieces());

        let files_size = torrent.files_total_size();
        let piece_length = torrent.meta.info.piece_length as usize;

        let last_piece_length = files_size % piece_length;
        let last_piece_length = if last_piece_length == 0 { piece_length } else { last_piece_length };

        if piece_length == 0 {
            panic!("Invalid piece length");
        }

        let num_pieces = (files_size + piece_length - 1) / piece_length;

        if sha1_pieces.len() != num_pieces {
            panic!("Invalid hashes");
        }

        let block_size = std::cmp::min(piece_length, 0x4000);
        let nblocks_piece = (piece_length + block_size - 1) / block_size;
        let nblocks_last_piece = ((files_size % piece_length) + block_size - 1) / block_size;

        Pieces {
            num_pieces,
            sha1_pieces,
            block_size,
            nblocks_piece,
            nblocks_last_piece,
            piece_length,
            last_piece_length,
            files_size,
        }
    }
}

//use bit_field::BitArray;

// TODO:
// See https://doc.rust-lang.org/1.29.0/core/arch/x86_64/fn._popcnt64.html
// https://github.com/rust-lang/packed_simd
// https://stackoverflow.com/questions/42938907/is-it-possible-to-use-simd-instructions-in-rust

impl Pieces {
    // fn update(&mut self, update: &BitFieldUpdate)  {
    //     match update {
    //         BitFieldUpdate::BitField(bitfield) => {
    //             for (i, piece) in self.peers_pieces.iter_mut().enumerate() {
    //                 if bitfield.get_bit(i) {
    //                     *piece = piece.saturating_add(1);
    //                 }
    //             }
    //         }
    //         BitFieldUpdate::Piece(piece) => {
    //             // TODO: Handle inexistant piece
    //             if let Some(piece) = self.peers_pieces.get_mut(*piece) {
    //                 *piece = piece.saturating_add(1);
    //             }
    //         }
    //     }
    // }

    fn piece_size(&self, piece_index: usize) -> usize {
        if piece_index == self.num_pieces - 1 {
            self.last_piece_length
        } else {
            self.piece_length
        }
    }
}

// type PeerAddr = Sender<MessageActor>;

/// Data shared between peers and torrent actor
#[derive(Debug)]
struct SharedData {
    torrent: Torrent,
}

impl SharedData {
    fn new(torrent: Torrent) -> SharedData {
        SharedData {
            torrent,
        }
    }
}

#[derive(Debug, Clone)]
struct TorrentData(Arc<RwLock<SharedData>>);

impl TorrentData {
    fn new(torrent: Torrent) -> TorrentData {
        TorrentData(Arc::new(RwLock::new(
            SharedData::new(torrent)
        )))
    }

    fn clone(&self) -> TorrentData {
        TorrentData(Arc::clone(&self.0))
    }

    fn with<R, F>(&self, mut fun: F) -> R
    where
        F: FnMut(&SharedData) -> R
    {
        let data = self.0.read();
        fun(&data)
    }

    fn with_write<R, F>(&self, fun: F) -> R
    where
        F: Fn(&mut SharedData) -> R
    {
        let mut data = self.0.write();
        fun(&mut data)
    }

    fn read(&self) -> RwLockReadGuard<SharedData> {
        self.0.read()
    }
}

use async_std::sync as a_sync;

enum PeerMessage {
    AddPeer {
        id: PeerId,
        queue: PeerTask,
        addr: a_sync::Sender<PeerCmd>
    },
    RemovePeer {
        id: PeerId ,
        queue: PeerTask
    },
    AddPiece(PieceBuffer),
    UpdateBitfield {
        id: PeerId,
        update: BitFieldUpdate
    }
}

impl std::fmt::Debug for PeerMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        use PeerMessage::*;
        match self {
            AddPeer { id, .. } => {
                f.debug_struct("PeerMessage")
                 .field("AddPeer", &id)
                 .finish()
            }
            RemovePeer { id, .. } => {
                f.debug_struct("PeerMessage")
                 .field("RemovePeer", &id)
                 .finish()
            }
            AddPiece(piece) => {
                f.debug_struct("PeerMessage")
                 .field("AddPiece", &piece.piece_index)
                 .finish()
            }
            UpdateBitfield { id, .. } => {
                f.debug_struct("PeerMessage")
                 .field("UpdateBitfield", &id)
                 .finish()
            }
        }
    }
}

struct PeerState {
    bitfield: BitField,
    queue_tasks: PeerTask,
    addr: a_sync::Sender<PeerCmd>
}

struct TorrentSupervisor {
    data: TorrentData,
    trackers: Vec<Tracker>,
    receiver: a_sync::Receiver<PeerMessage>,
    // We keep a Sender to not close the channel
    // in case there is no peer
    _sender: a_sync::Sender<PeerMessage>,

    pieces_detail: Pieces,

    peers: Map<PeerId, PeerState>,

    pieces: Vec<Option<PieceInfo>>,
}

pub type Result<T> = std::result::Result<T, TorrentError>;

use std::sync::Arc;
use parking_lot::{RwLock, RwLockReadGuard};

impl TorrentSupervisor {
    fn new(torrent: Torrent) -> TorrentSupervisor {
        let (_sender, receiver) = a_sync::channel(100);
        let pieces_detail = Pieces::from(&torrent);

        let num_pieces = pieces_detail.num_pieces;
        let mut pieces = Vec::with_capacity(num_pieces);
        pieces.resize_with(num_pieces, Default::default);

        TorrentSupervisor {
            data: TorrentData::new(torrent),
            receiver,
            _sender,
            pieces_detail,
            pieces,
            peers: Default::default(),
            trackers: vec![],
            //queue_map: Default::default()
        }
    }

    async fn start(&mut self) {
        self.collect_trackers();

        if let Some(addrs) = self.find_tracker() {
            self.connect_to_peers(&addrs, self.data.clone());
        }

        self.process_cmds().await;
    }

    fn collect_trackers(&mut self) {
        let trackers = self.data.with(|data| {
            data.torrent.iter_urls().map(Tracker::new).collect()
        });
        self.trackers = trackers;
    }

    fn connect_to_peers(&self, addrs: &[SocketAddr], data: TorrentData) {
        for addr in addrs {
            println!("ADDR: {:?}", addr);

            let addr = *addr;
            let data = data.clone();
            let sender = self._sender.clone();
            let pieces_detail = self.pieces_detail.clone();

            task::spawn(async move {
                let mut peer = match Peer::new(addr, data, pieces_detail, sender).await {
                    Ok(peer) => peer,
                    Err(e) => {
                        println!("PEER ERROR {:?}", e);
                        return;
                    }
                };
                peer.start().await;
            });
        }
    }

    fn find_tracker(&mut self) -> Option<Vec<SocketAddr>> {
        let data = self.data.read();
        let torrent = &data.torrent;

        loop {
            for tracker in &mut self.trackers {
                println!("TRYING {:?}", tracker);
                match tracker.announce(&torrent) {
                    Ok(peers) => return Some(peers),
                    Err(e) => {
                        eprintln!("[Tracker announce] {:?}", e);
                        continue;
                    }
                };
            }
        }
        None
    }

    async fn process_cmds(&mut self) {
        use PeerMessage::*;

        while let Some(msg) = self.receiver.recv().await {
            match msg {
                UpdateBitfield { id, update } => {
                    if self.find_pieces_for_peer(id, &update).await {
                        let peer = self.peers.get(&id).unwrap();
                        peer.addr.send(PeerCmd::TasksAvailables).await;
                    }

                    if let Some(peer) = self.peers.get_mut(&id) {
                        peer.bitfield.update(update);
                    };
                }
                RemovePeer { id, queue } => {
                    self.peers.remove(&id);

                    for piece in self.pieces.iter_mut().filter_map(Option::as_mut) {
                        piece.workers.retain(|p| !Arc::ptr_eq(&p, &queue) );
                    }
                }
                AddPeer { id, queue, addr } => {
                    self.peers.insert(id, PeerState {
                        bitfield: BitField::new(self.pieces_detail.num_pieces),
                        queue_tasks: queue,
                        addr
                    });
                }
                AddPiece (piece_block) => {
                    let index = piece_block.piece_index;
                    let sha1_torrent = self.pieces_detail.sha1_pieces.get(index).map(Arc::clone);

                    if let Some(sha1_torrent) = sha1_torrent {
                        let sha1 = Sha1::from(&piece_block.buf).digest();
                        let sha1 = sha1.bytes();
                        if sha1 == sha1_torrent.as_slice() {
                            //println!("SHA1 ARE GOOD !! {}", piece_block.piece_index);
                        } else {
                            println!("WRONG SHA1 :() {}", piece_block.piece_index);
                        }
                    } else {
                        println!("PIECE RECEIVED BUT NOT FOUND {}", piece_block.piece_index);
                    }

                    println!("[S] PIECE RECEIVED {} {}", piece_block.piece_index, piece_block.buf.len());
                }
            }
        }
    }

    async fn find_pieces_for_peer(&mut self, peer: PeerId, update: &BitFieldUpdate) -> bool {
        let mut pieces = &mut self.pieces;
        let nblock_piece = self.pieces_detail.nblocks_piece;
        let block_size = self.pieces_detail.block_size;

        let queue_peer = self.peers.get_mut(&peer).map(|p| &mut p.queue_tasks).unwrap();
        let mut queue = queue_peer.write().await;

        let mut found = false;

        match update {
            BitFieldUpdate::BitField(bitfield) => {
                let pieces = pieces.iter_mut()
                                   .enumerate()
                                   .filter(|(index, p)| p.is_none() && bitfield.get_bit(*index))
                                   .take(20);

                for (piece, value) in pieces {
                    for i in 0..nblock_piece {
                        queue.push_back(PieceToDownload::new(piece, i * block_size, block_size));
                    }
                    //println!("[{:?}] PUSHING PIECE={}", peer.id, piece);
                    value.replace(PieceInfo::new(queue_peer.clone()));
                    if !found {
                        found = true;
                    }
                }
            }
            BitFieldUpdate::Piece(piece) => {
                let piece = *piece;

                if piece >= pieces.len() {
                    return false;
                }

                if pieces.get(piece).unwrap().is_none() {
                    for i in 0..nblock_piece {
                        queue.push_back(PieceToDownload::new(piece, i * block_size, block_size));
                    }
                    //println!("[{:?}] _PUSHING PIECE={}", peer.id, piece);
                    pieces.get_mut(piece).unwrap().replace(PieceInfo::new(queue_peer.clone()));
                    found = true;
                }

            }
        }

        found
    }
}

enum FilesMessage {
    HaveBitfield,
    HavePiece { piece_index: u32 }
}

struct FilesActor {
    data: TorrentData,
}

struct SessionInner {
    cmds: Receiver<SessionCommand>,
    actors: Vec<TorrentSupervisor>
}

use async_std::task;

impl SessionInner {
    fn start(&self) {
        task::block_on(async {
            self.start_session()
        });
    }

    fn start_session(&self) {
        for cmd in self.cmds.iter() {
            self.dispatch(cmd);
        }
    }

    fn dispatch(&self, cmd: SessionCommand) {
        use SessionCommand::*;

        match cmd {
            AddTorrent(torrent) => {
                task::spawn(async {
                    TorrentSupervisor::new(torrent).start().await;
                });
            }
        }
    }
}

enum SessionCommand {
    AddTorrent(Torrent)
}

pub struct Session {
    handle: std::thread::JoinHandle<()>,
    actor: Sender<SessionCommand>,
}

impl Session {
    pub fn new() -> Session {
        let (sender, receiver) = unbounded();

        let handle = std::thread::spawn(move || {
            let session = SessionInner {
                cmds: receiver,
                actors: vec![]
            };
            session.start();
        });

        Session { handle, actor: sender }
    }

    pub fn add_torrent(&mut self, torrent: Torrent) {
        self.actor.send(SessionCommand::AddTorrent(torrent));
    }
}
