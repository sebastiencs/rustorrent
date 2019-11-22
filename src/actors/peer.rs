use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use futures::future::{Fuse, FutureExt};
use async_std::net::TcpStream;
use async_std::io::{BufReader, BufWriter};
use async_std::prelude::*;
use async_std::sync as a_sync;
use coarsetime::{Duration, Instant};

use std::pin::Pin;
use std::io::{Cursor, Write, Read};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::convert::TryFrom;
use std::net::SocketAddr;

use crate::bitfield::{BitField, BitFieldUpdate};
use crate::utils::Map;
use crate::pieces::{Pieces, PieceToDownload, PieceBuffer};
use crate::supervisors::torrent::{PeerMessage, Result};
use crate::errors::TorrentError;

static PEER_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub type PeerId = usize;

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

impl<'a> TryFrom<&'a [u8]> for MessagePeer<'a> {
    type Error = TorrentError;

    fn try_from(buffer: &'a [u8]) -> Result<MessagePeer> {
        if buffer.len() == 0 {
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

#[derive(Debug, PartialEq, Eq)]
enum Choke {
    UnChoked,
    Choked
}

use std::collections::VecDeque;

pub type PeerTask = Arc<async_std::sync::RwLock<VecDeque<PieceToDownload>>>;

#[derive(Debug)]
pub enum PeerCommand {
    TasksAvailables
}

enum PeerWaitEvent {
    Peer(Result<usize>),
    Supervisor(Option<PeerCommand>),
}

pub struct Peer {
    id: PeerId,
    addr: SocketAddr,
    supervisor: a_sync::Sender<PeerMessage>,
    reader: BufReader<TcpStream>,
    buffer: Vec<u8>,
    /// Are we choked from the peer
    choked: Choke,
    /// BitField of the peer
    bitfield: BitField,
    /// List of pieces to download
    tasks: PeerTask,
    _tasks: Option<VecDeque<PieceToDownload>>,

    /// Small buffers where the downloaded blocks are kept.
    /// Once the piece is full, we send it to TorrentSupervisor
    /// and remove it here.
    pieces_buffer: Map<usize, PieceBuffer>,

    pieces_detail: Pieces,

    nblocks: usize, // Downloaded
    start: Option<Instant>, // Downloaded,
}

impl Peer {
    pub async fn new(
        addr: SocketAddr,
        pieces_detail: Pieces,
        supervisor: a_sync::Sender<PeerMessage>,
    ) -> Result<Peer> {
        let stream = TcpStream::connect(&addr).await?;

        let id = PEER_COUNTER.fetch_add(1, Ordering::SeqCst);

        let bitfield = BitField::new(pieces_detail.num_pieces);

        Ok(Peer {
            addr,
            supervisor,
            pieces_detail,
            bitfield,
            id,
            tasks: PeerTask::default(),
            reader: BufReader::with_capacity(32 * 1024, stream),
            buffer: Vec::with_capacity(32 * 1024),
            choked: Choke::Choked,
            nblocks: 0,
            start: None,
            _tasks: None,
            pieces_buffer: Map::default(),
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
        mut cmds: Pin<&mut Fuse<impl Future<Output = Option<PeerCommand>>>>
    ) -> PeerWaitEvent {
        // use futures::async_await::*;
        use futures::task::{Context, Poll};
        use futures::{future, pin_mut};

        let mut msgs = self.read_messages();
        pin_mut!(msgs); // Pin on the stack

        // assert_unpin(&msgs);
        // assert_unpin(&cmds);
        // assert_fused_future();

        let mut fun = |cx: &mut Context<'_>| {
            match FutureExt::poll_unpin(&mut msgs, cx).map(PeerWaitEvent::Peer) {
                v @ Poll::Ready(_) => return v,
                _ => {}
            }

            match FutureExt::poll_unpin(&mut cmds, cx).map(PeerWaitEvent::Supervisor) {
                v @ Poll::Ready(_) => return v,
                _ => Poll::Pending
            }
        };

        future::poll_fn(fun).await
    }

    pub async fn start(&mut self) -> Result<()> {

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
            use PeerWaitEvent::*;

            match self.wait_event(cmds.as_mut()).await {
                Peer(Ok(n)) => {
                    self.dispatch().await?;
                }
                Supervisor(command) => {
                    use PeerCommand::*;

                    match command {
                        Some(TasksAvailables) => {
                            self.maybe_send_request().await;
                        }
                        None => {
                            // Disconnected
                        }
                    }
                }
                Peer(Err(e)) => {
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
            },
            BitField (bitfield) => {
                // Send an Interested ?

                let bitfield = crate::bitfield::BitField::from(
                    bitfield,
                    self.pieces_detail.num_pieces
                )?;

                let update = BitFieldUpdate::from(bitfield);

                self.supervisor.send(PeerMessage::UpdateBitfield { id: self.id, update }).await;

                println!("[{:?}] BITFIELD", self.id);
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

                self.pieces_buffer
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
        let piece_buffer = self.pieces_buffer.remove(&(index as usize)).unwrap();

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
        cursor.write(self.pieces_detail.info_hash.as_ref());
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
