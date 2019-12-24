
#![allow(
    dead_code,
    clippy::new_without_default,
    clippy::single_match,
    clippy::large_enum_variant
)]

use std::io::{self, Read};

use rustorrent::session::Session;
use rustorrent::de;
use rustorrent::utp;

use async_std::net::{SocketAddr, IpAddr, Ipv4Addr};

// use rustorrent::cache_line::CacheAligned;
//use rustorrent::memory_pool::page::Block;

//use rustorrent::memory_pool::pool::CircularIterator;

//fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
fn main() {

    // let mut vec = vec![1, 2, 3, 4, 5, 6];

    // for i in vec.circular_iter_mut(7) {
    //     println!("I={:?}", i);
    // }

    // return;

    // struct Dumb {
    //     b: usize,
    //     a: [CacheAligned<usize>; 1500]
    // }

    // struct Page2<T> {
    //     must_deallocate: CacheAligned<AtomicBool>,

    //     bitfield: [CacheAligned<AtomicU64>; 32],

    //     pages: [CacheAligned<Block<T>>; 32],
    // }

    // let size = std::mem::size_of::<Page2<Dumb>>();

    // println!("SIZE BOOL {:?}",
    //          std::mem::size_of::<CacheAligned<AtomicBool>>()
    //          + std::mem::size_of::<CacheAligned<AtomicU64>>() * 32
    //          + std::mem::size_of::<CacheAligned<Block<Dumb>>>() * 32
    // );
    // println!("SIZE {:?}", size);

    // println!("Align {:?}", std::mem::align_of::<Dumb>());
    // println!("Align {:?}", std::mem::align_of::<Page2<Dumb>>());

    // return;

    // use std::sync::atomic::AtomicUsize;

    // let mut array = Vec::with_capacity(64 * 1024 * 1024);
    // array.resize_with(64 * 1024 * 1024, || {
    //     AtomicUsize::new(0)
    // });

    // for step in 1..1024 {

    //     let now = std::time::Instant::now();
    //     for i in 0..(array.len() / step) {
    //         let elem = &array[i * step];
    //         let n = elem.load(Ordering::Relaxed);

    //         elem.compare_exchange(n, n * 3, Ordering::SeqCst, Ordering::Relaxed);
    //     }
    //     let end = std::time::Instant::now() - now;

    //     println!("STEP_{}: {:?}", step, end);

    // }

    // let now = std::time::Instant::now();
    // for i in 0..array.len() {
    //     array[i].fetch_add(1, Ordering::SeqCst);
    // }
    // let end = std::time::Instant::now() - now;

    // println!("STEP_1: {:?}", end);

    // let now = std::time::Instant::now();
    // for i in 0..(array.len() / 3) {
    //     array[i * 3].fetch_add(1, Ordering::SeqCst);
    // }
    // let end = std::time::Instant::now() - now;

    // println!("STEP_3: {:?}", end);


//     use rustorrent::memory_pool::Arena;

//     #[derive(Copy, Clone)]
//     struct MyStruct {
//         a: Option<usize>,
//         b: &'static str,
//         c: usize
//     }

//     impl Default for MyStruct {
//         fn default() -> MyStruct {
//             MyStruct {
//                 a: None,
//                 b: "hello",
//                 c: 90909
//             }
//         }
//     }


//     let now;
//     {
//         let mut arena = Arena::<MyStruct>::with_capacity(100000000);
//         let obj = MyStruct::default();

//         // println!("ICIII", );

//         now = std::time::Instant::now();

//         for _ in 0..100000000 {
//             let res = arena.alloc(MyStruct::default());
//             std::mem::forget(res);
//         }
//     }

//     let end = std::time::Instant::now() - now;

//     // println!("LAAA", );

//     println!("STAT: {:?}", end / 100000000);
// //    println!("STAT: {:?} {:?}", arena.stats(), end / 100000000);

//     return;

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

    // use std::sync::atomic::{AtomicU16, Ordering};

    // let foo = AtomicU16::new(u16::max_value());
    // assert_eq!(foo.fetch_add(10, Ordering::SeqCst), 0);

    // println!("FOO {:?}", foo.fetch_add(1, Ordering::SeqCst));
    // println!("FOO {:?}", foo.load(Ordering::SeqCst));

    async_std::task::block_on(async {
        // let listener = utp::stream::UtpListener::new(10001).await.unwrap();
        // let listener = Arc::new(listener);
        // let listener2 = Arc::clone(&listener);
        // listener.start();

        // let stream = listener.connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 131)), 7000)).await.unwrap();
        //let stream = listener.connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7000)).await.unwrap();

        let stdin = io::stdin();
        let mut buffer = Vec::new();
        let mut handle = stdin.lock();

        handle.read_to_end(&mut buffer).unwrap();

        let listener = utp::stream::UtpListener::new(10001).await.unwrap();
        // let listener = Arc::new(listener);
        // let listener2 = Arc::clone(&listener);
        // listener.start();

        let listener_clone = listener.clone();
        let buffer_clone = buffer.clone();

        async_std::task::spawn(async move {
            let stream = listener_clone.connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7001)).await.unwrap();
            stream.write(&buffer_clone).await;
        });

        let stream = listener.connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7000)).await.unwrap();
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
