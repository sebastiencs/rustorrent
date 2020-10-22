use crate::metadata::Torrent;
//use crate::http_client::{self, AnnounceQuery, AnnounceResponse};

//use crate::http_client::HttpError;
use crossbeam_channel::{unbounded, Receiver as SyncReceiver, Sender as SyncSender};

// enum MessageActor {
//     AddPeer(PeerAddr),
//     RemovePeer(PeerAddr),
// }

// type PeerAddr = Sender<MessageActor>;
use crate::supervisors::torrent::TorrentSupervisor;

use crate::actors::sha1::{Sha1Workers, Sha1Task};

struct SessionInner {
    cmds: SyncReceiver<SessionCommand>,
    actors: Vec<TorrentSupervisor>,
    sha1_workers: SyncSender<Sha1Task>
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
    actor: SyncSender<SessionCommand>,
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
        self.actor
            .send(SessionCommand::AddTorrent(torrent))
            .expect("Error contacting session");
    }
}
