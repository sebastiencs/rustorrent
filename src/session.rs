use crate::metadata::Torrent;
use crate::http_client::{self, AnnounceQuery, AnnounceResponse};

struct TorrentActor {
    torrent: Torrent
}

impl TorrentActor {
    fn start(&self) {
        self.connect();
    }
    
    fn connect(&self) {
        for url in self.torrent.iter_urls() {
            println!("URL={:?}", url);
            let torrent = &self.torrent;
            let query = AnnounceQuery::from(torrent);

            let res: Result<AnnounceResponse,_> = http_client::get(url, query);
            // println!("URL: {:?}", url);
            println!("RESPONSE: {:?}", res);
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
