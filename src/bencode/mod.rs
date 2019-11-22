
use std::net::{SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr};
use std::io::Cursor;
use byteorder::BigEndian;
use byteorder::ReadBytesExt;
use serde::{de, Deserializer, Deserialize};
use serde::de::Visitor;

pub mod ser;

#[derive(Debug)]
pub struct CompactIpv6(Vec<SocketAddrV6>);

/// Deserialize compact IPV6

impl<'de> Deserialize<'de> for CompactIpv6 {
    fn deserialize<D>(deserializer: D) -> Result<CompactIpv6, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Ipv6Visitor;
        impl<'de> Visitor<'de> for Ipv6Visitor {
            type Value = CompactIpv6;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an integer a compact ipv6")
            }

            fn visit_bytes<E>(self, bin: &[u8]) -> Result<CompactIpv6, E>
            where
                E: de::Error,
            {
                let mut addrs = Vec::with_capacity(bin.len() / 18);
                for chunk in bin.chunks_exact(18) {
                    let mut cursor = Cursor::new(chunk);
                    let mut addr: [u16; 9] = [0; 9];
                    for i in 0..9 {
                        match cursor.read_u16::<BigEndian>() {
                            Ok(value) => addr[i] = value,
                            _ => continue
                        }
                    }
                    let ipv6 = Ipv6Addr::new(addr[0], addr[1], addr[2], addr[3], addr[4], addr[5], addr[6], addr[7]);
                    let ipv6 = SocketAddrV6::new(ipv6, addr[8], 0, 0);
                    addrs.push(ipv6);
                }
                Ok(CompactIpv6(addrs))
            }
        }
        deserializer.deserialize_bytes(Ipv6Visitor)
    }
}

/// Deserialize compact IPV4

#[derive(Debug)]
pub struct CompactIpv4(Vec<SocketAddrV4>);

impl<'de> Deserialize<'de> for CompactIpv4 {
    fn deserialize<D>(deserializer: D) -> Result<CompactIpv4, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Ipv4Visitor;
        impl<'de> Visitor<'de> for Ipv4Visitor {
            type Value = CompactIpv4;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an integer a compact ipv4")
            }

            fn visit_bytes<E>(self, bin: &[u8]) -> Result<CompactIpv4, E>
            where
                E: de::Error,
            {
                let mut addrs = Vec::with_capacity(bin.len() / 6);
                for chunk in bin.chunks_exact(6) {
                    let mut cursor = Cursor::new(&chunk[..]);
                    let ipv4 = match cursor.read_u32::<BigEndian>() {
                        Ok(ipv4) => ipv4,
                        _ => continue
                    };
                    let port = match cursor.read_u16::<BigEndian>() {
                        Ok(port) => port,
                        _ => continue
                    };
                    addrs.push(SocketAddrV4::new(Ipv4Addr::from(ipv4), port));
                }
                Ok(CompactIpv4(addrs))
            }
        }
        deserializer.deserialize_bytes(Ipv4Visitor)
    }
}
