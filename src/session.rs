use crate::metadata::Torrent;
use crate::http_client::{self, AnnounceQuery, AnnounceResponse};
use crate::bitfield::BitField;
use crate::utils::{FromSlice, Map};

use crate::http_client::{Peers,Peers6};
use std::io::Cursor;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

use crate::actors::peer::Peer;
use crate::pieces::Pieces;
use crate::pieces::PieceToDownload;
use crate::supervisors::torrent::Result;

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
pub struct Tracker {
    pub url: Url,
    pub announce: Option<AnnounceResponse>
}

impl Tracker {
    pub fn new(url: Url) -> Tracker {
        Tracker { url, announce: None }
    }

    pub fn announce(&mut self, torrent: &Torrent) -> Result<Vec<SocketAddr>> {
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

use sha1::Sha1;

use async_std::sync::Mutex;

use async_std::prelude::*;

use std::borrow::Cow;

use std::thread;

// enum MessageActor {
//     AddPeer(PeerAddr),
//     RemovePeer(PeerAddr),
// }

// type PeerAddr = Sender<MessageActor>;
use crate::supervisors::torrent::TorrentSupervisor;
use async_std::sync as a_sync;

use crate::actors::sha1::{Sha1Workers, Sha1Task};

struct SessionInner {
    cmds: Receiver<SessionCommand>,
    actors: Vec<TorrentSupervisor>,
    sha1_workers: Sender<Sha1Task>
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
                let sha1_workers = self.sha1_workers.clone();
                task::spawn(async move {
                    TorrentSupervisor::new(torrent, sha1_workers).start().await;
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
        let sha1_workers = Sha1Workers::new_pool();

        let handle = std::thread::spawn(move || {
            let session = SessionInner {
                cmds: receiver,
                actors: vec![],
                sha1_workers,
            };
            session.start();
        });

        Session { handle, actor: sender }
    }

    pub fn add_torrent(&mut self, torrent: Torrent) {
        self.actor.send(SessionCommand::AddTorrent(torrent));
    }
}
