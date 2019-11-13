use serde::{Serialize, Deserialize};

mod de;

#[derive(Debug, Serialize, Deserialize)]
struct Info {
    #[serde(rename="piece length")]
    piece_length: i64,
    //length: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Torrent {
    announce: String,
    info: Info,
    #[serde(rename="announce-list")]
    announce_list: Option<Vec<Vec<String>>>,
    #[serde(rename="creation date")]
    creation_date: Option<u64>,
    comment: Option<String>,
    #[serde(rename="created by")] 
    created_by: Option<String>,
    encoding: Option<String>
}

use async_std::task;

use std::collections::HashMap;
use std::io::{self, Read};

//fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
fn main() {
    let stdin = io::stdin();
    let mut buffer = Vec::new();
    let mut handle = stdin.lock();
    match handle.read_to_end(&mut buffer) {
        Ok(_) => {
            match de::from_bytes::<Torrent>(&buffer) {
                Ok(t) => println!("RES: {:#?}", t),
                Err(e) => println!("ERROR: {:?}", e),
            }
        }
        Err(e) => println!("ERROR: {:?}", e),

    }
    
    // task::block_on(async {
    //     let mut res = surf::get("https://httpbin.org/get").await?;
    //     //res.body_json();
    //     println!("{:#?}", res.body_json().await?);
    //     // dbg!(res.body_string().await?);
    //     Ok(())
    // })
}
