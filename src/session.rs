use crate::metadata::Torrent;
use crate::http_client::{self, AnnounceQuery, AnnounceResponse};

struct TorrentActor {
    torrent: Torrent
}

use crate::http_client::{Peers,Peers6};
use std::io::Cursor;
use byteorder::{BigEndian, ReadBytesExt};
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

struct Peer {
    
}

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
                
                let ipv4 = Ipv4Addr::from(
                    cursor.read_u32::<BigEndian>().unwrap(),
                );
                
                let port = match cursor.read_u16::<BigEndian>() {
                    Ok(port) => port,
                    _ => continue
                };
                
                let ipv4 = SocketAddrV4::new(ipv4, port);

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
use std::net::TcpStream;
use smallvec::SmallVec;
use std::io::BufReader;

fn read_messages(mut stream: TcpStream) -> Result<(), std::io::Error> {
    let mut buffer = Vec::with_capacity(32_768);

    let mut i = 0;
    loop {
        let stream = std::io::Read::by_ref(&mut stream);
        
        match stream.take(4).read_to_end(&mut buffer) {
            Ok(0) => return Ok(()),
            Err(e) => return Ok(()),
            _ => {}
        }

        let length = {
            // println!("LEN BUF = {:?}", buffer.len());
            let mut cursor = Cursor::new(&buffer[..]);
            cursor.read_u32::<BigEndian>()? as u64
        };

        if length == 0 {
            continue;
        } // else if length >= buffer.capacity() {
        //     buffer.reserve(buffer.capacity() - length);
        // }

        stream.take(length).read_to_end(&mut buffer)?;
        //stream.read_exact(&mut buffer[..length]);

        match buffer[0] {
            0 => println!("CHOKE", ),
            1 => println!("UNCHOKE", ),
            2 => println!("INTERESTED", ),
            3 => println!("NOT INTERESTED", ),
            4 => {
                //cursor.set_position(1);
                let mut cursor = Cursor::new(&buffer[1..]);
                println!("HAVE {:?}", cursor.read_u32::<BigEndian>()?);
            }
            5 => {
                println!("BITFIELD {:?}", &buffer[1..]);
            }
            6 => {
                println!("REQUEST {:?}", &buffer[1..]);
            }
            x => { println!("UNKNOWN {} {:?}", x, &buffer[1..]); }
        }
        i += 1;
        if i >= 6 {
            return Ok(())
        }
    }
}

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

fn do_handshake(addr: &SocketAddr, torrent: &Torrent) -> Result<(), std::io::Error> {
    let mut stream = TcpStream::connect_timeout(addr, std::time::Duration::from_secs(5))?;

    let mut handshake: [u8; 68] = [0; 68];

    {
        let mut cursor = Cursor::new(&mut handshake[..]);

        cursor.write(&[19]);
        cursor.write(b"BitTorrent protocol");
        cursor.write(&[0,0,0,0,0,0,0,0]);
        cursor.write(torrent.info_hash.as_ref());
        cursor.write(b"-RT1220sJ1Nna5rzWLd8");
    }

    stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    
    stream.write_all(&handshake)?;
    stream.flush()?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));

    // TODO: Use SmallVec here
    let mut buffer = [0; 128];

    stream.read_exact(&mut buffer[..1]);

    let len = buffer[0] as usize;

    stream.read_exact(&mut buffer[..len + 48]);

    if &buffer[len + 8..len + 28] == torrent.info_hash.as_slice() {
        //println!("HASH MATCHED !", );
    }

    read_messages(stream)
}

impl TorrentActor {
    fn start(&self) {
        self.connect();
    }
    
    fn connect(&self) {
        for url in self.torrent.iter_urls().filter(|url| url.scheme() == "http") {
            println!("URL={:?}", url);
            let torrent = &self.torrent;
            let query = AnnounceQuery::from(torrent);

            let res: Result<AnnounceResponse,_> = http_client::get(url, query);

            if let Ok(ref res) = res {
                let addrs = get_peers_addrs(res);

                for addr in &addrs {
                    println!("ADDR: {:?}", addr);
                    let res = do_handshake(addr, torrent);
                    println!("RES: {:?}", res);
                }
            };


            // println!("URL: {:?}", url);
            //println!("RESPONSE: {:?}", res);
        }
    }
}

pub struct Session {
    actors: Vec<TorrentActor>
}

impl Session {
    pub fn new() -> Session {
        Session { actors: vec![] }
    }
    
    pub fn add_torrent(&mut self, torrent: Torrent) {
        println!("TORRENT={:#?}", torrent);

        let actor = TorrentActor { torrent };

        actor.start();

        // let query = AnnounceQuery::from(&torrent);

        // let res: AnnounceResponse = http_client::get(&torrent.meta.announce, query).unwrap();

        // println!("RESPONSE: {:?}", res);
    }
}
