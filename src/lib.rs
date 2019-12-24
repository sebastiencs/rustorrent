
#![allow(
    dead_code,
    clippy::new_without_default,
    clippy::single_match,
    clippy::large_enum_variant
)]

pub mod metadata;
pub mod session;
pub mod bitfield;
pub mod utils;
pub mod actors;
pub mod pieces;
pub mod supervisors;
pub mod errors;
pub mod extensions;
pub mod bencode;
pub mod udp_ext;
pub mod sha1;
pub mod utp;
pub mod time;
pub mod cache_line;
pub mod memory_pool;

//https://blog.cloudflare.com/how-to-receive-a-million-packets/
