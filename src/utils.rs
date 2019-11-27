
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
    fn write(&mut self, _: &[u8]) {
        unreachable!()
    }
    fn write_usize(&mut self, n: usize) {
        self.0 = n
    }
}

pub type Map<K, V> = std::collections::HashMap<K, V, NoHash>;

use async_std::net::SocketAddr;
use byteorder::{BigEndian, ReadBytesExt};
use std::io::Cursor;
use std::net::{SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr};

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
