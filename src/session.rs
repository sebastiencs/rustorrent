use crate::metadata::Torrent;
use crate::http_client::{self, AnnounceQuery, AnnounceResponse};

// enum State {
//     Handshaking,
//     Downloading
// }

use crate::http_client::{Peers,Peers6};
use std::io::Cursor;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

// struct Peers {
//     addrs: Vec<SocketAddr>
// }

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
//use std::net::TcpStream;
use smallvec::SmallVec;
//use std::io::{BufReader, BufWriter};

// fn read_messages(mut stream: TcpStream) -> std::result::Result<(), std::io::Error> {
//     let mut buffer = Vec::with_capacity(32_768);

//     //let mut stream = BufReader::with_capacity(32_768, stream);

//     let mut i = 0;
//     loop {
//         let stream = std::io::Read::by_ref(&mut stream);

//         println!("READING LENGTH", );
        
//         buffer.clear();
//         match stream.take(4).read_to_end(&mut buffer) {
//             Ok(0) => return Ok(()),
//             Err(e) => {
//                 println!("ERROR: {:?}", e);
//                 return Ok(());
//             }
//             _ => {}
//         }

//         let length = {
//             // println!("LEN BUF = {:?}", buffer.len());
//             let mut cursor = Cursor::new(&buffer[..]);
//             cursor.read_u32::<BigEndian>()? as u64
//         };
        
//         println!("LENGTH={} {:?}", length, &buffer[..]);

//         if length == 0 {
//             continue;
//         } // else if length >= buffer.capacity() {
//         //     buffer.reserve(buffer.capacity() - length);
//         // }

//         buffer.clear();

//         stream.take(length).read_to_end(&mut buffer)?;
//         //stream.read_exact(&mut buffer[..length]);

//         println!("ICIIII", );

//         let mut last_have = 0;

//         match buffer[0] {
//             0 => {
//                 println!("CHOKE {:?} {:?}", String::from_utf8_lossy(&buffer[1..]), &buffer[..]);
//                 // let mut aa: [u8; 5] = [0; 5];
//                 // let mut cursor = Cursor::new(&mut aa[..]);
//                 // cursor.write_u32::<BigEndian>(1)?;
//                 // cursor.write_u8(2)?;
                                
//                 // stream.write_all(&aa)?;
//                 // stream.flush()?;

//                 // println!("INTERESTED SENT");

//                 // // request: <len=0013><id=6><index><begin><length>
                
//                 // let mut aa: [u8; 13] = [0; 13];
//                 // let mut cursor = Cursor::new(&mut aa[..]);
//                 // cursor.write_u32::<BigEndian>(13)?;
//                 // cursor.write_u8(6)?;
//                 // cursor.write_u32::<BigEndian>(0)?;
//                 // cursor.write_u32::<BigEndian>(256)?;
                                
//                 // stream.write_all(&aa)?;
//                 // stream.flush()?;

//                 // println!("REQUEST SENT");
//             }
//             1 => {
//                 println!("UNCHOKE", );
                
//                 let mut aa: [u8; 17] = [0; 17];
//                 let mut cursor = Cursor::new(&mut aa[..]);
//                 cursor.write_u32::<BigEndian>(13)?;
//                 cursor.write_u8(6)?;
//                 cursor.write_u32::<BigEndian>(last_have)?;
//                 cursor.write_u32::<BigEndian>(0)?;
//                 cursor.write_u32::<BigEndian>(16384)?;
                                
//                 stream.write_all(&aa)?;
//                 stream.flush()?;

//                 println!("REQUEST SENT");
//             }
//             2 => println!("INTERESTED", ),
//             3 => println!("NOT INTERESTED", ),
//             4 => {
//                 //cursor.set_position(1);
//                 let mut cursor = Cursor::new(&buffer[1..]);
//                 last_have = cursor.read_u32::<BigEndian>()?;
//                 println!("HAVE {:?}", last_have);
//             }
//             5 => {
//                 println!("BITFIELD {:?}", &buffer[1..]);
                
//                 let mut aa: [u8; 5] = [0; 5];
//                 let mut cursor = Cursor::new(&mut aa[..]);
//                 cursor.write_u32::<BigEndian>(1)?;
//                 cursor.write_u8(2)?;
                                
//                 stream.write_all(&aa)?;
//                 stream.flush()?;

//                 println!("INTERESTED SENT");
//             }
//             6 => {
//                 println!("REQUEST {:?}", &buffer[1..]);
//             }
//             7 => {
//                 // piece: <len=0009+X><id=7><index><begin><block>
                
//                 let mut cursor = Cursor::new(&buffer[1..]);

//                 let index = cursor.read_u32::<BigEndian>()?;
//                 let begin = cursor.read_u32::<BigEndian>()?;
                
//                 println!("PIECE ! {:?} {:?}", index, begin);
//             }
//             x => { println!("UNKNOWN {} {:?}", x, &buffer[1..]); }
//         }
//         i += 1;
//         // if i >= 6 {
//         //     return Ok(())
//         // }
//     }
// }

// fn read_messages(mut stream: TcpStream) {
//     //let mut buffer = [0; 4096];
//     let mut buffer = Vec::with_capacity(4096);
//     //let mut buffer = BufReader::new(stream);
//     //let mut cursor = Cursor::new(&buffer[..]);
    
//     loop {
// //        buffer.read_exact();
//         stream.read_exact(&mut buffer[..4]);

//         let length = {
//             let mut cursor = Cursor::new(&buffer[..]);
//             cursor.read_u32::<BigEndian>().unwrap() as usize
//         };

//         if length == 0 {
//             continue;
//         } else if length >= buffer.capacity() {
//             buffer.reserve(buffer.capacity() - length);
//         }
        
//         stream.read_exact(&mut buffer[..length]);

//         match buffer[0] {
//             0 => println!("CHOKE", ),
//             1 => println!("UNCHOKE", ),
//             2 => println!("INTERESTED", ),
//             3 => println!("NOT INTERESTED", ),
//             4 => {
//                 //cursor.set_position(1);
//                 let mut cursor = Cursor::new(&buffer[1..]);
//                 println!("HAVE {:?}", cursor.read_u32::<BigEndian>());
//             }
//             5 => {
//                 println!("BITFIELD", );
//             }
//             x => { println!("UNKNOWN {}", x); }
//         }
//     }
// }

//fn do_handshake(addr: &SocketAddr, torrent: &Torrent) -> std::result::Result<(), std::io::Error> {
// fn do_handshake(addr: &SocketAddr, torrent: &Torrent) {
//     let mut stream = TcpStream::connect_timeout(addr, std::time::Duration::from_secs(5))?;

//     let mut handshake: [u8; 68] = [0; 68];

//     {
//         let mut cursor = Cursor::new(&mut handshake[..]);

//         cursor.write(&[19]);
//         cursor.write(b"BitTorrent protocol");
//         cursor.write(&[0,0,0,0,0,0,0,0]);
//         cursor.write(torrent.info_hash.as_ref());
//         cursor.write(b"-RT1220sJ1Nna5rzWLd8");
//     }

//     stream.set_write_timeout(Some(std::time::Duration::from_secs(30)));
    
//     stream.write_all(&handshake)?;
//     stream.flush()?;
//     stream.set_read_timeout(Some(std::time::Duration::from_secs(30)));

//     // TODO: Use SmallVec here
//     let mut buffer = [0; 128];

//     stream.read_exact(&mut buffer[..1]);

//     let len = buffer[0] as usize;

//     stream.read_exact(&mut buffer[..len + 48]);

//     if &buffer[len + 8..len + 28] == torrent.info_hash.as_slice() {
//         //println!("HASH MATCHED !", );
//     }

//     //read_messages(stream)
// }

use url::Url;

// struct TrackersList {
//     list: Vec<Tracker>
// }

// impl From<&Torrent> for TrackersList {
//     fn from(torrent: &Torrent) -> TrackersList {
//         TrackersList {
//             list: torrent.iter_urls().map(Tracker::new).collect()
//         }
//     }
// }

use crate::http_client::HttpError;
use crossbeam_channel::{unbounded, Receiver, Sender};

#[derive(Debug)]
enum TorrentError {
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

enum PeerState {
    Connecting,
    Handshaking,
    Downloading {
        piece: usize,
        index: usize,
    },
    Dead
}

enum Choke {
    UnChoked,
    Choked
}

struct Peer {
    addr: SocketAddr,
    data: TorrentData,
    reader: BufReader<TcpStream>,
    state: PeerState,
    buffer: Vec<u8>,
    /// Are we choked from the peer
    choked: Choke,
}

use async_std::sync::Mutex;

use async_std::net::TcpStream;
use async_std::prelude::*;
use async_std::io::{BufReader, BufWriter};

trait FromSlice<T> {
    fn from_slice(size: usize, slice: &[T]) -> Vec<T>;
}

impl<T: Copy> FromSlice<T> for Vec<T> {
    fn from_slice(size: usize, slice: &[T]) -> Vec<T> {
        let mut vec = Vec::with_capacity(size);
        unsafe { vec.set_len(size); }
        vec.as_mut_slice().copy_from_slice(slice);
        vec
    }
}

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

impl Peer {
    async fn new(addr: SocketAddr, data: TorrentData) -> Result<Peer> {
        let stream = TcpStream::connect(&addr).await?;
        
        Ok(Peer {
            addr,
            data,
            reader: BufReader::with_capacity(32 * 1024, stream),
            state: PeerState::Connecting,
            buffer: Vec::with_capacity(32 * 1024),
            choked: Choke::Choked,
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

    async fn start(&mut self) -> Result<()> {
        self.do_handshake().await?;

        loop {
            self.read_messages().await?;
            self.dispatch().await?;
        }
        
        Ok(())
    }

    fn set_choked(&mut self, choked: bool) {
        self.choked = if choked {
            Choke::Choked
        } else {
            Choke::UnChoked
        };
    }

    async fn dispatch<'a>(&'a mut self) -> Result<()> {
        use MessagePeer::*;

        let msg = MessagePeer::try_from(self.buffer.as_slice())?;
        
        match msg {
            Choke => {
                self.set_choked(true);
                println!("CHOKE", );
            },
            UnChoke => {
                // If the peer has piece we're interested in
                // Send a Request
                self.set_choked(false);
                println!("UNCHOKE", );                
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
                // Update peer info
                // Send an Interested ?
                self.data.with_write(|data| {
                    data.pieces.update_have(piece_index);
                });
                println!("HAVE {}", piece_index);
            },
            BitField (bitfield) => {
                // Send an Interested ?
                self.data.with_write(|data| {
                    data.pieces.update_from_bitfield(bitfield);
                });
                self.data.with(|data| {
                    println!("BITFIELD {:?}", data.pieces.have_pieces);
                });
                //println!("BITFIELD {:?}", bitfield.get(0..10));
            },
            Request { index, begin, length } => {
                // Mark this peer as interested
                // Make sure this peer is not choked, send a choke
                println!("REQUEST {} {} {}", index, begin, length);
            },
            Piece { index, begin, block } => {
                // If we already have it, send another Request
                // Check the sum and write to disk
                // Send Request
                println!("PIECE {} {} {:?}", index, begin, block);
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

    async fn read_messages(&mut self) -> Result<()> {
        self.read_exactly(4).await?;
        
        let length = {
            let mut cursor = Cursor::new(&self.buffer);
            cursor.read_u32::<BigEndian>()? as usize
        };
        
        // println!("LENGTH={} {:?}", length, &self.buffer[..]);

        if length == 0 {
            return Ok(()); // continue
        }
        
        self.read_exactly(length).await?;

        Ok(())

        // let mut last_have = 0;

        // let (id, buffer) = (self.buffer[0], &self.buffer[..]);

        // return MessagePeer::try_from(self.buffer.as_slice());

        //println!("MESSAGE {:?}", message);
        
        // match id {
        //     0 => {
        //         println!("CHOKE {:?} {:?}", String::from_utf8_lossy(&buffer[1..]), &buffer[..]);
        //     }
        //     1 => {
        //         println!("UNCHOKE", );
                
        //         let mut aa: [u8; 17] = [0; 17];
        //         let mut cursor = Cursor::new(&mut aa[..]);
        //         cursor.write_u32::<BigEndian>(13)?;
        //         cursor.write_u8(6)?;
        //         cursor.write_u32::<BigEndian>(last_have)?;
        //         cursor.write_u32::<BigEndian>(0)?;
        //         cursor.write_u32::<BigEndian>(16384)?;
                
        //         // stream.write_all(&aa)?;
        //         // stream.flush()?;

        //         println!("REQUEST SENT");
        //     }
        //     2 => println!("INTERESTED", ),
        //     3 => println!("NOT INTERESTED", ),
        //     4 => {
        //         //cursor.set_position(1);
        //         let mut cursor = Cursor::new(&buffer[1..]);
        //         last_have = cursor.read_u32::<BigEndian>()?;
        //         println!("HAVE {:?}", last_have);
        //     }
        //     5 => {
        //         println!("BITFIELD {:?}", &buffer[1..]);
                
        //         let mut aa: [u8; 5] = [0; 5];
        //         let mut cursor = Cursor::new(&mut aa[..]);
        //         cursor.write_u32::<BigEndian>(1)?;
        //         cursor.write_u8(2)?;
                
        //         // stream.write_all(&aa)?;
        //         // stream.flush()?;

        //         println!("INTERESTED SENT");
        //     }
        //     6 => {
        //         println!("REQUEST {:?}", &buffer[1..]);
        //     }
        //     7 => {
        //         // piece: <len=0009+X><id=7><index><begin><block>
                
        //         let mut cursor = Cursor::new(&buffer[1..]);

        //         let index = cursor.read_u32::<BigEndian>()?;
        //         let begin = cursor.read_u32::<BigEndian>()?;
                
        //         println!("PIECE ! {:?} {:?}", index, begin);
        //     }
        //     x => { println!("UNKNOWN {} {:?}", x, &buffer[1..]); }
        // }
        //     i += 1;
        //     // if i >= 6 {
        //     //     return Ok(())
        //     // }
        // }
        // Ok(())
    }

    // fn loop_on_messages(&mut self) -> Result<()> {
        
    // }

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

        // TODO: Check the info hash and send to other TorrentActor if necessary

        println!("DONE", );

        Ok(())
    }

}

enum MessageActor {
    AddPeer(PeerAddr),
    RemovePeer(PeerAddr),
}

#[derive(Debug)]
struct Pieces {
    /// Number of pieces
    num_pieces: usize,
    /// SHA1 of each piece
    sha1_pieces: Vec<Arc<Vec<u8>>>,
    /// Pieces other peers have
    /// have_pieces[0] is the number of peers having the piece 0
    have_pieces: Vec<u8>,
    /// Size of a block
    block_size: usize,
    /// Number of block in 1 piece
    nblocks_piece: usize,
    /// Number of block in the last piece
    nblocks_last_piece: usize,
}

impl From<&Torrent> for Pieces {
    fn from(torrent: &Torrent) -> Pieces {
        let sha1_pieces = torrent.sha_pieces();

        let total = torrent.files_total_size();
        let piece_length = torrent.meta.info.piece_length as usize;

        if piece_length == 0 {
            panic!("Invalid piece length");
        }
        
        let num_pieces = (total + piece_length - 1) / piece_length;

        if sha1_pieces.len() != num_pieces {
            panic!("Invalid hashes");
        }
        
        let mut have_pieces = Vec::with_capacity(num_pieces);
        have_pieces.resize_with(num_pieces, || 0);
        
        let block_size = std::cmp::min(piece_length, 0x4000);
        let nblocks_piece = (piece_length + block_size - 1) / block_size;
        let nblocks_last_piece = ((total % piece_length) + block_size - 1) / block_size;
        
        Pieces {
            num_pieces,
            sha1_pieces,
            have_pieces,
            block_size,
            nblocks_piece,
            nblocks_last_piece
        }
    }
}

//use bit_field::BitArray;

// TODO:
// See https://doc.rust-lang.org/1.29.0/core/arch/x86_64/fn._popcnt64.html
// https://github.com/rust-lang/packed_simd
// https://stackoverflow.com/questions/42938907/is-it-possible-to-use-simd-instructions-in-rust

impl Pieces {
    fn update_from_bitfield(&mut self, bitfield: &[u8]) {
        // TODO: Handle bitfield with wrong length
        if bitfield.len() * 8 >= self.have_pieces.len() {
            for (i, value) in self.have_pieces.iter_mut().enumerate() {
                let slice_index = i / 8;
                let bit_index = i % 8;

                if bitfield[slice_index] & (1 << (7 - bit_index)) != 0 {
                    *value = value.saturating_add(1);                
                }
            }
        }
    }

    fn update_have(&mut self, piece_num: u32) {
        // TODO: Handle inexistant piece
        if let Some(piece) = self.have_pieces.get_mut(piece_num as usize) {
            *piece = piece.saturating_add(1);
        }
    }
}

type PeerAddr = Sender<MessageActor>;

/// Data shared between peers and torrent actor
#[derive(Debug)]
struct SharedData {
    torrent: Torrent,
    pieces: Pieces
}

impl SharedData {
    fn new(torrent: Torrent) -> SharedData {
        SharedData {
            pieces: Pieces::from(&torrent),
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

struct TorrentActor {
    data: TorrentData,
    peers: Vec<PeerAddr>,
    trackers: Vec<Tracker>,
    receiver: Receiver<MessageActor>,
    // We keep a Sender to not close the channel
    // in case there is no peer
    _sender: Sender<MessageActor>,
}

type Result<T> = std::result::Result<T, TorrentError>;

use std::sync::Arc;
use parking_lot::{RwLock, RwLockReadGuard};

impl TorrentActor {
    fn new(torrent: Torrent) -> TorrentActor {
        let (_sender, receiver) = unbounded();
        TorrentActor {
            data: TorrentData::new(torrent),
            receiver,
            _sender,
            peers: vec![],
            trackers: vec![],            
        }
    }
    
    fn start(&mut self) {
        self.collect_trackers();
        
        if let Some(addrs) = self.find_tracker() {
            self.connect_to_peers(&addrs, self.data.clone());
        }
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
            
            task::spawn(async move {
                let mut peer = match Peer::new(addr, data).await {
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
}

struct SessionInner {
    cmds: Receiver<SessionCommand>,
    actors: Vec<TorrentActor>    
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
                    TorrentActor::new(torrent).start();
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
