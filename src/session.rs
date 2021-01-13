use std::sync::Arc;

use crate::{
    fs::{standard_fs::StandardFS, uring_fs::UringFS, FSMessage, FileSystem},
    listener::{Listener, ListenerMessage},
    logger,
    metadata::Torrent,
};
//use crate::http_client::{self, AnnounceQuery, AnnounceResponse};

//use crate::http_client::HttpError;
use async_channel::Sender;
use crossbeam_channel::{unbounded, Receiver as SyncReceiver, Sender as SyncSender};

use tokio::runtime::Runtime;
// enum MessageActor {
//     AddPeer(PeerAddr),
//     RemovePeer(PeerAddr),
// }

// type PeerAddr = Sender<MessageActor>;
use crate::supervisors::torrent::TorrentSupervisor;

use crate::actors::sha1::{Sha1Task, Sha1Workers};

struct SessionInner {
    cmds: SyncReceiver<SessionCommand>,
    actors: Vec<TorrentSupervisor>,
    sha1_workers: SyncSender<Sha1Task>,
    fs: Sender<FSMessage>,
    runtime: Arc<Runtime>,
    listener: Sender<ListenerMessage>,
}

impl SessionInner {
    fn start(&self) {
        self.runtime.block_on(async { self.start_session().await })
    }

    async fn start_session(&self) {
        for cmd in self.cmds.iter() {
            self.dispatch(cmd);
        }
    }

    fn dispatch(&self, cmd: SessionCommand) {
        use SessionCommand::*;

        match cmd {
            AddTorrent(torrent) => {
                let sha1_workers = self.sha1_workers.clone();
                let fs = self.fs.clone();
                let listener = self.listener.clone();
                tokio::spawn(async move {
                    TorrentSupervisor::new(torrent, sha1_workers, fs, listener)
                        .start()
                        .await;
                });
            }
        }
    }
}

enum SessionCommand {
    AddTorrent(Torrent),
}

pub struct Session {
    handle: std::thread::JoinHandle<()>,
    actor: SyncSender<SessionCommand>,
    runtime: Arc<Runtime>,
}

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}

impl Session {
    pub fn new() -> Session {
        logger::start();

        let (sender, receiver) = unbounded();
        let runtime = Arc::new(Runtime::new().unwrap());
        let fs = match UringFS::init(runtime.clone()) {
            Some(fs) => fs,
            _ => StandardFS::new(runtime.clone()),
        };
        let sha1_workers = Sha1Workers::new_pool(runtime.clone(), fs.clone());
        let listener = Listener::new(runtime.clone());
        let runtime_clone = runtime.clone();

        let handle = std::thread::spawn(move || {
            let session = SessionInner {
                cmds: receiver,
                actors: vec![],
                sha1_workers,
                runtime: runtime_clone,
                fs,
                listener,
            };
            session.start();
        });

        Session {
            handle,
            actor: sender,
            runtime,
        }
    }

    pub fn add_torrent(&mut self, torrent: Torrent) {
        self.actor
            .send(SessionCommand::AddTorrent(torrent))
            .expect("Error contacting session");
    }
}
