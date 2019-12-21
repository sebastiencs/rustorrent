use serde::{Serialize, Deserialize};
// use smallvec::SmallVec;
use async_std::task;

// mod de;
// mod metadata;
// mod session;
// mod bitfield;
// mod utils;
// mod actors;
// mod pieces;
// mod supervisors;
// mod errors;
// mod extensions;
// mod bencode;
// mod udp_ext;

// use async_std::task;
use std::io::{self, Read};
use std::sync::Arc;

use rustorrent::session::Session;
use rustorrent::de;
use rustorrent::sha1::sha1;
use rustorrent::utp;

use async_std::net::{SocketAddr, IpAddr, Ipv4Addr};

//fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
fn main() {

    // Acquire: load
    // Release: store

    // println!("1 << 16 = {}", 1 << 16);
    // println!("2 * (1 << 16) = {}", 2 * (1 << 16));
    // println!("(2 * (1 << 16)) >> 16 = {}", (2 * (1 << 16)) >> 16);
    // println!("397 * (1 << 16) = {}", 397 * (1 << 16));
    // println!("(397 * (1 << 16)) >> 16 = {}", (397 * (1 << 16)) >> 16);
    // println!("397 / 2 = {}", 397 / 2);
    // println!("(397 * (1 << 16)) / 2 = {}", (397 * (1 << 16)) / 2);
    // println!("((397 * (1 << 16)) / 2) >> 16 = {}", ((397 * (1 << 16)) / 2) >> 16);

    use std::sync::atomic::{AtomicU16, Ordering};

    let foo = AtomicU16::new(u16::max_value());
    // assert_eq!(foo.fetch_add(10, Ordering::SeqCst), 0);

    println!("FOO {:?}", foo.fetch_add(1, Ordering::SeqCst));
    println!("FOO {:?}", foo.load(Ordering::SeqCst));

    async_std::task::block_on(async {
        let mut listener = utp::stream::UtpListener::new(10001).await.unwrap();
        // let listener = Arc::new(listener);
        // let listener2 = Arc::clone(&listener);
        // listener.start();

        let stream = listener.connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 131)), 7000)).await.unwrap();
        //let stream = listener.connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7000)).await.unwrap();

        let stdin = io::stdin();
        let mut buffer = Vec::new();
        let mut handle = stdin.lock();

        handle.read_to_end(&mut buffer).unwrap();

        stream.write(&buffer).await;

         // stream.write(b"hello").await;

        println!("DOOOOONE", );

        // return Ok(true);


        // let mut socket = utp::socket::UtpSocket::bind(
        //     SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080)
        // ).await.unwrap();
        // socket.connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7000)).await.unwrap();

        let stdin = io::stdin();
        let mut buffer = Vec::new();
        let mut handle = stdin.lock();

        handle.read_to_end(&mut buffer).unwrap();

        // socket.send(&buffer).await.unwrap();

        // socket.send(b"hello weshaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").await.unwrap();
        // socket.send(b"OKLM").await.unwrap();
        // socket.send(b"OUIOUIOUIOUIOUIOUIOUIOUI").await.unwrap();
    });

    return ;

    let stdin = io::stdin();
    let mut buffer = Vec::new();
    let mut handle = stdin.lock();

    handle.read_to_end(&mut buffer).unwrap();

    //let (meta, info) = de::from_bytes_with_hash::<MetaTorrent>(&buffer).unwrap();
    let torrent = de::read_meta(&buffer).unwrap();

    println!("TORRENT={:#?}", torrent);

    let mut session = Session::new();

    session.add_torrent(torrent);

    let mut buffer = String::new();
    let stdin = io::stdin();
    let mut handle = stdin.lock();

    handle.read_to_string(&mut buffer).unwrap();
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
