use serde::{Serialize, Deserialize};
use smallvec::SmallVec;

mod de;
mod http_client;
mod metadata;

use metadata::MetaTorrent;

use async_std::task;

use std::collections::HashMap;
use std::io::{self, Read};

use http_client::Escaped;

#[derive(Serialize, Deserialize, Debug)]
pub struct Query {
    info_hash: Vec<u8>,
    peer_id: String,
    port: i64,
    uploaded: i64,
    downloaded: i64,
    //left: i64,
    event: String,
    compact: i64
}

//fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
fn main() {
    let stdin = io::stdin();
    let mut buffer = Vec::new();
    let mut handle = stdin.lock();

    handle.read_to_end(&mut buffer).unwrap();

    let (meta, info) = de::from_bytes::<MetaTorrent>(&buffer).unwrap();

    println!("META: {:#?}\nINFO={:?}", meta, info);

    let query = Query {
        info_hash: info,
        peer_id: "-RT1220sJ1Nna5rzWLd8".to_owned(),
        port: 6881,
        uploaded: 0,
        downloaded: 0,
        event: "started".to_owned(),
        compact: 1,
    };
    
    http_client::get(meta.announce, query);
    //http_client::get("http://localhost:90/announce", query);
    
    
//     task::block_on(async move {
//         let mut res = surf::get("http://localhost:6969/announce")
// //        let mut res = surf::get(&meta.announce)
//             .set_query(&query)?
//             .recv_string()
//             .await?;

//         println!("{:#?}", res);

//         Ok(())
//     })
}
