#![allow(
    dead_code,
    // clippy::new_without_default,
    // clippy::single_match,
)]

pub mod actors;
pub mod bencode;
pub mod bitfield;
pub mod cache_line;
pub mod errors;
pub mod extensions;
pub mod fs;
pub mod io_uring;
pub mod logger;
pub mod metadata;
pub mod peer;
pub mod piece_collector;
pub mod piece_picker;
pub mod pieces;
pub mod session;
pub mod sha1;
pub mod spsc;
pub mod supervisors;
pub mod time;
pub mod udp_ext;
pub mod utils;
pub mod utp;

// pub mod memory_pool;

//https://blog.cloudflare.com/how-to-receive-a-million-packets/
