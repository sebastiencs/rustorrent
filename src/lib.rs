#![allow(
    dead_code,
    clippy::new_without_default,
    clippy::single_match,
    clippy::large_enum_variant
)]

pub mod actors;
pub mod bencode;
pub mod bitfield;
pub mod cache_line;
pub mod errors;
pub mod extensions;
pub mod logger;
pub mod metadata;
pub mod pieces;
pub mod session;
pub mod sha1;
pub mod supervisors;
pub mod time;
pub mod udp_ext;
pub mod utils;
pub mod utp;

// pub mod memory_pool;

//https://blog.cloudflare.com/how-to-receive-a-million-packets/
