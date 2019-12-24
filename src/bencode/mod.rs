
pub mod ser;
pub mod de;

use serde::de::Visitor;
use serde::{de as serde_de, Deserializer, Deserialize};

#[derive(Debug)]
pub struct PtrBuf<'a> {
    pub slice: &'a [u8]
}

impl<'a, 'de: 'a> Deserialize<'de> for PtrBuf<'a> {
    fn deserialize<D>(deserializer: D) -> Result<PtrBuf<'a>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PtrBufVisitor;
        impl<'de> Visitor<'de> for PtrBufVisitor {
            type Value = PtrBuf<'de>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("Expecting a buffer")
            }
            fn visit_borrowed_bytes<E>(self, slice: &'de [u8]) -> Result<PtrBuf<'de>, E>
            where
                E: serde_de::Error,
            {
                Ok(PtrBuf { slice })
            }
        }
        deserializer.deserialize_bytes(PtrBufVisitor)
    }
}
