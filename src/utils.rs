
use async_std::net::SocketAddr;
use async_std::net::TcpStream;
use async_trait::async_trait;
use async_std::io;
use byteorder::{BigEndian, ReadBytesExt};

use std::time::Duration;
use std::io::Cursor;
use std::net::{SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr};
use std::convert::TryInto;

pub trait FromSlice<T> {
    fn from_slice(slice: &[T]) -> Vec<T>;
}

impl<T: Copy> FromSlice<T> for Vec<T> {
    fn from_slice(slice: &[T]) -> Vec<T> {
        let len = slice.len();
        let mut vec = Vec::with_capacity(len);
        unsafe { vec.set_len(len); }
        vec.as_mut_slice().copy_from_slice(slice);
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
}

/// A map, without hashing
pub type Map<K, V> = std::collections::HashMap<K, V, NoHash>;

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
    async fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> io::Result<TcpStream>;
}

#[async_trait]
impl ConnectTimeout for TcpStream {
    async fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        io::timeout(timeout, async move { TcpStream::connect(addr).await }).await
    }
}
