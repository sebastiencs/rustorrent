#![allow(
    dead_code,
    clippy::new_without_default,
    clippy::single_match,
    clippy::large_enum_variant
)]

use std::{
    env,
    io::{self, Read},
};

use rustorrent::{bencode::de, session::Session};

#[allow(unreachable_code)]
#[tokio::main]
async fn main() {
    let file = match env::args().nth(1) {
        Some(file) => file,
        _ => {
            env!("CARGO_MANIFEST_DIR").to_owned()
                + "/scripts/Fedora-Workstation-Live-x86_64-33.torrent"
        }
    };

    // let file = env!("CARGO_MANIFEST_DIR").to_owned()
    //     + "/scripts/Fedora-Workstation-Live-x86_64-33.torrent";

    // let file = "/home/sebastien/Downloads/Fedora-Workstation-Live-x86_64-33.torrent";
    // let file = "/home/sebastien/Downloads/Fedora-Workstation-Live-x86_64-33_Beta.torrent";
    // let file = "/home/sebastien/Downloads/ubuntu-20.10-desktop-amd64.iso.torrent";
    let buffer = std::fs::read(file).unwrap();

    let torrent = de::read_meta(&buffer).unwrap();

    println!("TORRENT={:#?}", torrent);

    let mut session = Session::new();

    session.add_torrent(torrent);

    let mut buffer = String::new();
    let stdin = io::stdin();
    let mut handle = stdin.lock();

    handle.read_to_string(&mut buffer).unwrap();
}
