use async_channel::{Sender, TrySendError};
use async_trait::async_trait;
use byteorder::{BigEndian, ReadBytesExt};
use std::net::SocketAddr;
use tokio::net::TcpStream;

use std::{
    io::Cursor,
    net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6},
    time::Duration,
};

pub trait FromSlice<T> {
    fn from_slice(slice: &[T]) -> Vec<T>;
}

impl<T: Copy> FromSlice<T> for Vec<T> {
    fn from_slice(slice: &[T]) -> Vec<T> {
        let mut vec = Vec::new();
        vec.extend(slice);
        vec
    }
}

#[derive(Default)]
pub struct NoHash(usize);

impl std::hash::BuildHasher for NoHash {
    type Hasher = Self;
    fn build_hasher(&self) -> Self::Hasher {
        Self(0)
    }
}

impl std::hash::Hasher for NoHash {
    fn finish(&self) -> u64 {
        self.0 as u64
    }
    fn write(&mut self, _slice: &[u8]) {
        panic!("Wrong use of NoHash");
    }
    fn write_usize(&mut self, n: usize) {
        self.0 = n
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = n as usize
    }
    fn write_u32(&mut self, n: u32) {
        self.0 = n as usize
    }
    fn write_u16(&mut self, n: u16) {
        self.0 = n as usize
    }
}

/// A map, without hashing
pub type Map<K, V> = std::collections::HashMap<K, V, NoHash>;

/// A set, without hashing
pub type Set<V> = std::collections::HashSet<V, NoHash>;

pub(crate) fn send_to<T>(sender: &Sender<T>, msg: T)
where
    T: Send + 'static,
{
    if let Err(TrySendError::Full(msg)) = sender.try_send(msg) {
        let sender = sender.clone();
        tokio::spawn(async move { sender.send(msg).await });
    }
}

pub fn ipv4_from_slice(slice: &[u8], output: &mut Vec<SocketAddr>) {
    for chunk in slice.chunks_exact(6) {
        let mut cursor = Cursor::new(&chunk[..]);
        let ipv4 = cursor.read_u32::<BigEndian>().unwrap();
        let port = cursor.read_u16::<BigEndian>().unwrap();
        output.push(SocketAddrV4::new(Ipv4Addr::from(ipv4), port).into());
    }
}

pub fn ipv6_from_slice(slice: &[u8], output: &mut Vec<SocketAddr>) {
    for chunk in slice.chunks_exact(18) {
        let mut cursor = Cursor::new(chunk);
        let mut addr: [u16; 8] = [0; 8];
        addr.iter_mut().for_each(|a| {
            *a = cursor.read_u16::<BigEndian>().unwrap();
        });
        let port = cursor.read_u16::<BigEndian>().unwrap();
        output.push(SocketAddrV6::new(Ipv6Addr::from(addr), port, 0, 0).into());
    }
}

#[async_trait]
pub trait ConnectTimeout {
    async fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> tokio::io::Result<TcpStream>;
}

#[async_trait]
impl ConnectTimeout for TcpStream {
    async fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> tokio::io::Result<TcpStream> {
        tokio::time::timeout(timeout, async move { TcpStream::connect(addr).await }).await?
    }
}

pub trait SaturatingDuration {
    fn saturating_duration_since(&self, earlier: coarsetime::Instant) -> coarsetime::Duration;
}

impl SaturatingDuration for coarsetime::Instant {
    fn saturating_duration_since(&self, earlier: coarsetime::Instant) -> coarsetime::Duration {
        // Avoid issues on platform where monotonic clock are wrong
        // https://github.com/rust-lang/rust/issues/56612
        let later = self.as_u64();
        let earlier = earlier.as_u64();
        let duration = later.saturating_sub(earlier);

        coarsetime::Duration::from_u64(duration)
    }
}
