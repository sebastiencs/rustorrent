
use serde::de::Visitor;
use serde::de::{DeserializeSeed, MapAccess, SeqAccess};
use serde::forward_to_deserialize_any;

use serde::Deserialize;

use std::cell::Cell;
use std::fmt;
use std::sync::Arc;

#[derive(Debug, PartialEq)]
pub enum DeserializeError {
    UnexpectedEOF,
    WrongCharacter(u8),
    End,
    InfoHashMissing,
    TooDeep,
    NoFile,
    EmptyFile,
    UnalignedPieces,
    Message(String)
}

type Result<T> = std::result::Result<T, DeserializeError>;

impl serde::de::Error for DeserializeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        DeserializeError::Message(msg.to_string())
    }
}

impl fmt::Display for DeserializeError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(&format!("{:?}", self))
        //formatter.write_str(std::error::Error::description(self))
    }
}

impl std::error::Error for DeserializeError {
    fn description(&self) -> &str {
        "aa"
        //self.msg.as_str()
    }
}

pub fn from_bytes<'de, T>(s: &'de [u8]) -> Result<T>
where
    T: Deserialize<'de>
{
    let mut de: Deserializer = Deserializer::new(s);
    T::deserialize(&mut de)
}

pub fn from_bytes_with_hash<'de, T>(s: &'de [u8]) -> Result<(T, Vec<u8>)>
where
    T: Deserialize<'de>
{
    let mut de: Deserializer = Deserializer::new(s);
    let res = T::deserialize(&mut de)?;

    let info_hash = if !de.start_info.is_null() && de.end_info > de.start_info {
        let len = de.end_info as usize - de.start_info as usize;
        let slice = unsafe { std::slice::from_raw_parts(de.start_info, len) };
        sha1::Sha1::from(&slice[..]).digest().bytes().to_vec()
    } else {
        //eprintln!("START={:?} END={:?}", de.start_info, de.end_info);

        return Err(DeserializeError::InfoHashMissing);
    };

    Ok((res, info_hash))
}

use crate::metadata::Torrent;
use crate::metadata::{InfoFile, MetaTorrent};

pub fn read_meta(s: &[u8]) -> Result<Torrent> {
    let (meta, info_hash): (MetaTorrent, Vec<u8>) = from_bytes_with_hash(s)?;

    if meta.info.pieces.len() % 20 != 0 {
        return Err(DeserializeError::UnalignedPieces);
    }

    match &meta.info.files {
        InfoFile::Multiple { files, .. } => {
            if files.is_empty() {
                return Err(DeserializeError::NoFile)
            }
        },
        InfoFile::Single { length, .. } => {
            if *length == 0 {
                return Err(DeserializeError::EmptyFile)
            }
        }
    }

    Ok(Torrent {
        meta,
        info_hash: Arc::new(info_hash),
    })
}

// 4b3ea6a5b1e62537dceb67230248ff092a723e4d
// 4b3ea6a5b1e62537dceb67230248ff092a723e4d

#[doc(hidden)]
pub struct Deserializer<'de> {
    input: &'de [u8],
    start_info: *const u8,
    end_info: *const u8,
    info_depth: i64,
    // Fix v2_deep_recursion.torrent
    depth: Cell<u16>,
}

#[doc(hidbn)]
impl<'de> Deserializer<'de> {
    fn new(input: &'de [u8]) -> Self {
        Deserializer {
            input,
            start_info: std::ptr::null(),
            end_info: std::ptr::null(),
            info_depth: 0,
            depth: Cell::new(0),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(0).copied()
    }

    fn next(&mut self) -> Result<u8> {
        if let Some(c) = self.peek() {
            let _ = self.consume();
            return Ok(c);
        }
        Err(DeserializeError::UnexpectedEOF)
    }

    fn consume(&mut self) -> Result<()> {
        self.input = &self.input
                          .get(1..)
                          .ok_or(DeserializeError::UnexpectedEOF)?;
        Ok(())
    }

    fn skip(&mut self, n: i64) -> Result<()> {
        self.input = &self.input
                          .get(n as usize..)
                          .ok_or(DeserializeError::UnexpectedEOF)?;
        Ok(())
    }

    fn read_integer(&mut self, stop: u8) -> Result<i64> {
        let mut n: i64 = 0;

        loop {
            match self.next()? {
                c @ b'0'..=b'9' => n = (n * 10) + (c - b'0') as i64,
                c if c == stop => break,
                c => return Err(DeserializeError::WrongCharacter(c))
            }
        }

        Ok(n)
    }

    fn read_number(&mut self) -> Result<i64> {
        self.consume()?; // 'i'

        let negative = match self.peek() {
            Some(b'-') => {
                self.consume()?;
                true
            },
            _ => false
        };

        let n = self.read_integer(b'e')?;

        Ok(if negative { -n } else { n })
    }

    fn read_string(&mut self) -> Result<&'de [u8]> {
        let len = self.read_integer(b':')?;

        let s = self.input.get(..len as usize)
                          .ok_or(DeserializeError::UnexpectedEOF)?;

        self.skip(len)?;

        if s == b"info" {
            //println!("INFO FOUND: {:?}", String::from_utf8(s.to_vec()));
            self.start_info = self.input.as_ptr();
            self.info_depth = 1;
        }

        //println!("STRING={:?}", String::from_utf8((&s[0..std::cmp::min(100, s.len())]).to_vec()));

        Ok(s)
    }
}

#[doc(hidden)]
impl<'a, 'de> serde::de::Deserializer<'de> for &'a mut Deserializer<'de> {
    type Error = DeserializeError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        // println!("NEXT: {:?}", self.peek());
        match self.peek().ok_or(DeserializeError::UnexpectedEOF)? {
            b'i' => {
                // println!("FOUND NUMBER", );
                visitor.visit_i64(self.read_number()?)
            }
            b'l' => {
                self.consume()?;
                // println!("FOUND LIST {:?}", &self.input[..10]);
                visitor.visit_seq(BencAccess::new(self))
            },
            b'd' => {
                let depth = self.depth.get();
                if depth > 100 {
                    return Err(DeserializeError::TooDeep);
                }
                self.depth.set(depth + 1);
                // println!("FOUND DICT {}", self.depth.get());
                self.consume()?;
                visitor.visit_map(BencAccess::new(self))
            },
            _n @ b'0'..=b'9' => {
                // println!("FOUND STRING", );
                visitor.visit_borrowed_bytes(self.read_string()?)
            }
            c => Err(DeserializeError::WrongCharacter(c))
        }
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_some(self)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string
        unit unit_struct seq tuple tuple_struct map struct identifier
        newtype_struct ignored_any enum bytes byte_buf
    }
}

struct BencAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>
}

impl<'a, 'de> BencAccess<'a, 'de> {
    fn new(de: &'a mut Deserializer<'de>) -> BencAccess<'a, 'de> {
        if de.info_depth >= 1 {
            de.info_depth += 1;
            let _s = de.input;
            //println!("DEPTH[NEW]={:?} {:?}", de.info_depth, String::from_utf8((&s[0..std::cmp::min(50, s.len())]).to_vec()));
        }
        BencAccess { de }
    }
}

impl<'a, 'de> MapAccess<'de> for BencAccess<'a, 'de> {
    type Error = DeserializeError;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>>
    where
        K: DeserializeSeed<'de>,
    {
        if self.de.peek() == Some(b'e') {
            let _ = self.de.consume();
            self.de.info_depth -= 1;
            if self.de.info_depth == 1 {
                //println!("FOUND END !");
                self.de.end_info = self.de.input.as_ptr();
            }
            let _s = self.de.input;
            //println!("DEPTH[END_DICT]={:?} {:?}", self.de.info_depth, String::from_utf8((&s[0..std::cmp::min(50, s.len())]).to_vec()));
            return Ok(None)
        }

        seed.deserialize(&mut *self.de).map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value>
    where
        V: DeserializeSeed<'de>,
    {
        seed.deserialize(&mut *self.de)
    }
}

impl<'a, 'de> SeqAccess<'de> for BencAccess<'a, 'de> {
    type Error = DeserializeError;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>>
    where
        T: DeserializeSeed<'de>,
    {
        // println!("LAAA {:?}", &self.de.input[..5]);

        if self.de.peek() == Some(b'e') {
            let _ = self.de.consume();
            if self.de.info_depth >= 1 {
                self.de.info_depth -= 1;
            }
            let _s = self.de.input;
            //println!("DEPTH[END_LIST]={:?} {:?}", self.de.info_depth, String::from_utf8((&s[0..std::cmp::min(50, s.len())]).to_vec()));
            // println!("DEPTH={}", self.de.info_depth);
            return Ok(None);
        }

        seed.deserialize(&mut *self.de).map(Some)
    }
}


#[allow(non_snake_case)]
#[cfg(test)]
mod tests {

    use super::{DeserializeError, from_bytes, Result};
    use serde::Deserialize;

    #[test]
    fn test_dict() {
        #[derive(Deserialize, PartialEq, Debug)]
        struct Dict<'b> {
            a: i64,
            b: &'b str,
            c: &'b str,
            X: &'b str,
        }

        let bc: Dict = from_bytes(b"d1:ai12453e1:b3:aaa1:c3:bbb1:X10:0123456789e").unwrap();

        assert_eq!(bc, Dict {
            a: 12453,
            b: "aaa",
            c: "bbb",
            X: "0123456789",
        });
    }

    #[test]
    fn test_key_no_value() {
        #[derive(Deserialize, PartialEq, Debug)]
        struct Dict<'b> {
            a: i64,
            b: &'b str,
        }

        let res: Result<Dict> = from_bytes(b"d1:ai1e1:be");

        println!("{:?}", res);
        assert_eq!(res, Err(DeserializeError::WrongCharacter(101)));
    }

    #[test]
    fn test_key_not_string() {
        #[derive(Deserialize, Debug)]
        struct Dict<'b> {
            a: i64,
            b: &'b str,
        }

        let res: Result<Dict> = from_bytes(b"di5e1:ae");

        println!("{:?}", res);
        assert!(res.is_err());
    }

    // TODO: Add more tests from
    // https://github.com/arvidn/libtorrent/blob/RC_1_2/test/test_bdecode.cpp
}
