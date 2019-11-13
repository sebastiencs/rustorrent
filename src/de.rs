
use serde;
use serde::de::Visitor;
use serde::de::{DeserializeOwned, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, Unexpected,
                VariantAccess};
use serde::forward_to_deserialize_any;

use serde::Deserialize;

// use crate::prelude::*;
use std::fmt;
use std::collections::VecDeque;

#[derive(Debug)]
pub enum DeserializeError {
    UnexpectedEOF,
    WrongCharacter(u8),
    End,
    Message(String)
}

// impl crate::error::JsError for DeserializeError {
//     fn get_msg(&self) -> String {
//         format!("DeserializeError {}", self.msg)
//     }
// }

// impl DeserializeError {
//     fn new() -> DeserializeError {
//         let msg = msg.as_ref();
//         DeserializeError { msg: msg.to_string() }
//     }
// }

// impl From<crate::Error> for DeserializeError {
//     fn from(o: crate::Error) -> DeserializeError {
//         DeserializeError {
//             msg: format!("{:?}", o)
//         }
//     }
// }

// impl From<Status> for DeserializeError {
//     fn from(o: Status) -> DeserializeError {
//         DeserializeError {
//             msg: format!("{:?}", o)
//         }
//     }
// }

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

// pub fn from_any<T>(env: Env, any: JsAny) -> Result<T>
// where
//     T: DeserializeOwned + ?Sized,
// {
//     let de: Deserializer = Deserializer::new(env, any);
//     T::deserialize(de)
// }

// pub fn from_value<T>(env: Env, value: Value) -> Result<T>
// where
//     T: DeserializeOwned + ?Sized,
// {
//     let de: Deserializer = Deserializer::new(env, JsAny::from(value)?);
//     T::deserialize(de)
// }

#[doc(hidden)]
pub struct Deserializer<'de> {
    input: &'de [u8]
}

#[doc(hidbn)]
impl<'de> Deserializer<'de> {
    fn new(input: &'de [u8]) -> Self {
        Deserializer { input }
    }

    fn peek(&self) -> Option<u8> {
        println!("PEEK {:?}", self.input.get(0..1));
        //self.input[..1].chars().next()
        //self.input.get(..1)?.chars().next()
        self.input.get(0).map(|p| *p)
    }

    fn next(&mut self) -> Result<u8> {
        if let Some(c) = self.peek() {
            self.consume();
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
                c @ b'0'...b'9' => n = (n * 10) + (c as i64 - b'0' as i64),
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

        println!("S: {:?}", s);
        
        self.skip(len)?;
        
        Ok(s)
    }
}

#[doc(hidden)]
impl<'a, 'de> serde::de::Deserializer<'de> for &'a mut Deserializer<'de> {
    type Error = DeserializeError;

    fn deserialize_any<V>(mut self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        println!("NEXT: {:?}", self.peek());
        match self.peek().ok_or(DeserializeError::UnexpectedEOF)? {
            b'i' => {                
                println!("FOUND NUMBER", );
                visitor.visit_i64(self.read_number()?)
            }
            b'l' => {
                self.consume()?;
                println!("FOUND LIST {:?}", &self.input[..10]);
                visitor.visit_seq(BencAccess::new(self))
            },
            b'd' => {
                println!("FOUND DICT", );
                self.consume()?;
                visitor.visit_map(BencAccess::new(self))
            },
//            'e' => Err(DeserializeError::End),
            n @ b'0'...b'9' => {
                println!("FOUND STRING", );                
                visitor.visit_borrowed_bytes(self.read_string()?)
            }
            c => Err(DeserializeError::WrongCharacter(c))
        }
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        println!("VISIT OPTION", );
        visitor.visit_some(self)
            // match self.input {
            //     JsAny::Undefined(_) | JsAny::Null(_) => visitor.visit_none(),
            //     _ => visitor.visit_some(self)
            // }
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        unimplemented!();
        // match self.input {
        //     JsAny::String(s) => {
        //         visitor.visit_enum(JsEnumAccess::new(self.env, s.to_rust().unwrap(), None))
        //     },
        //     JsAny::Object(o) => {
        //         let props = o.get_property_names()
        //                      .map_err(|e| DeserializeError::new(format!("{:?}", e)))?;

        //         if props.len() != 1 {
        //             return Err(DeserializeError::new(
        //                 format!("object with {} properties, expected 1", props.len())
        //             ));
        //         }
        //         let key = &props[0];
        //         let value = o.get(key.as_str())
        //                      .map_err(|e| DeserializeError::new(format!("{:?}", e)))?;

        //         visitor.visit_enum(JsEnumAccess::new(self.env, key.to_string(), Some(value)))
        //     },
        //     _ => {
        //         unimplemented!()
        //     }
        // }
    }

    fn deserialize_bytes<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        unimplemented!()
    }

    fn deserialize_byte_buf<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        unimplemented!()
    }

    // fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value>
    // where
    //     V: Visitor<'de>,
    // {
    //     //unimplemented!()
    //     self.deserialize_any(visitor)
    //     // println!("IGNORED {:?}", &self.input[..5]);
    //     // visitor.visit_unit()
    // }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string
        unit unit_struct seq tuple tuple_struct map struct identifier
        newtype_struct ignored_any
    }
}

struct BencAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>
}

impl<'a, 'de> BencAccess<'a, 'de> {
    fn new(de: &'a mut Deserializer<'de>) -> BencAccess<'a, 'de> {
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
            self.de.consume();
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
            self.de.consume();
            return Ok(None);
        }
        
        seed.deserialize(&mut *self.de).map(Some)
    }
}


// #[doc(hidden)]
// struct JsArrayAccess<'de> {
//     // env: Env,
//     // array: JsArray<'de>,
//     index: u32,
//     length: u32,
// }

// #[doc(hidden)]
// impl<'de> JsArrayAccess<'de> {
//     fn new(env: Env, array: JsArray<'de>) -> Self {
//         let length = array.len().unwrap() as u32;
//         JsArrayAccess {
//             // env,
//             // array,
//             index: 0,
//             length
//         }
//     }
// }

// #[doc(hidden)]
// impl<'de, 'de> SeqAccess<'de> for JsArrayAccess<'de> {
//     type Error = DeserializeError;

//     fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>>
//     where
//         T: DeserializeSeed<'de>,
//     {
//         if self.index >= self.length {
//             return Ok(None);
//         }
//         let value = self.array.get(self.index)?;
//         self.index += 1;

//         let de = Deserializer::new(self.env, value);
//         seed.deserialize(de).map(Some)
//     }

//     fn size_hint(&self) -> Option<usize> {
//         Some((self.length - self.index) as usize)
//     }
// }

// #[doc(hidden)]
// struct JsObjectAccess<'de> {
//     env: Env,
//     object: JsObject<'de>,
//     props: VecDeque<JsAny<'de>>,
// }

// #[doc(hidden)]
// impl<'de> JsObjectAccess<'de> {
//     fn new(env: Env, object: JsObject<'de>) -> Result<Self> {
//         let props = VecDeque::from(object.get_property_names_any().unwrap());

//         Ok(JsObjectAccess {
//             env,
//             object,
//             props,
//         })
//     }
// }

// #[doc(hidden)]
// impl<'de, 'de> MapAccess<'de> for JsObjectAccess<'de> {
//     type Error = DeserializeError;

//     fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>>
//     where
//         K: DeserializeSeed<'de>,
//     {
//         if self.props.is_empty() {
//             return Ok(None)
//         }

//         let prop = self.props.front().map(|v| v.clone()).unwrap();
//         let de = Deserializer::new(self.env, prop);
//         seed.deserialize(de).map(Some)
//     }

//     fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value>
//     where
//         V: DeserializeSeed<'de>,
//     {
//         if self.props.is_empty() {
//             panic!("Fetching value with empty props");
//         }
//         let key = self.props.pop_front().unwrap();
//         let value = self.object.get(key.get_value()).unwrap();

//         let de = Deserializer::new(self.env, value);
//         seed.deserialize(de)
//     }

//     fn next_entry_seed<K, V>(&mut self, kseed: K, vseed: V) -> Result<Option<(K::Value, V::Value)>>
//     where
//         K: DeserializeSeed<'de>,
//         V: DeserializeSeed<'de>,
//     {
//         if self.props.is_empty() {
//             return Ok(None);
//         }
//         let key = self.props.pop_front().unwrap();
//         let value = self.object.get(key.get_value()).unwrap();

//         let de = Deserializer::new(self.env, key);
//         let key = kseed.deserialize(de)?;

//         let de = Deserializer::new(self.env, value);
//         let value = vseed.deserialize(de)?;

//         Ok(Some((key, value)))
//     }

//     fn size_hint(&self) -> Option<usize> {
//         Some(self.props.len())
//     }
// }

// #[doc(hidden)]
// struct JsEnumAccess<'de> {
//     env: Env,
//     variant: String,
//     value: Option<JsAny<'de>>,
// }

// #[doc(hidden)]
// impl<'de> JsEnumAccess<'de> {
//     fn new(env: Env, key: String, value: Option<JsAny<'de>>) -> Self {
//         JsEnumAccess {
//             env,
//             variant: key,
//             value,
//         }
//     }
// }

// #[doc(hidden)]
// impl<'de, 'de> EnumAccess<'de> for JsEnumAccess<'de> {
//     type Error = DeserializeError;
//     type Variant = JsVariantAccess<'de>;

//     fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant)>
//     where
//         V: DeserializeSeed<'de>,
//     {
//         use serde::de::IntoDeserializer;
//         let variant = self.variant.into_deserializer();
//         let variant_access = JsVariantAccess::new(self.env, self.value);
//         seed.deserialize(variant).map(|v| (v, variant_access))
//     }
// }

// #[doc(hidden)]
// struct JsVariantAccess<'de> {
//     env: Env,
//     value: Option<JsAny<'de>>,
// }

// #[doc(hidden)]
// impl<'de> JsVariantAccess<'de> {
//     fn new(env: Env, value: Option<JsAny<'de>>) -> Self {
//         JsVariantAccess { env, value }
//     }
// }

// #[doc(hidden)]
// impl<'de, 'de> VariantAccess<'de> for JsVariantAccess<'de> {
//     type Error = DeserializeError;

//     fn unit_variant(self) -> Result<()> {
//         match self.value {
//             Some(val) => {
//                 let deserializer = Deserializer::new(self.env, val);
//                 serde::de::Deserialize::deserialize(deserializer)
//             }
//             None => Ok(()),
//         }
//     }

//     fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value>
//     where
//         T: DeserializeSeed<'de>,
//     {
//         match self.value {
//             Some(val) => {
//                 let deserializer = Deserializer::new(self.env, val);
//                 seed.deserialize(deserializer)
//             }
//             None => Err(serde::de::Error::invalid_type(
//                 Unexpected::UnitVariant,
//                 &"newtype variant",
//             )),
//         }
//     }

//     fn tuple_variant<V>(self, _len: usize, visitor: V) -> Result<V::Value>
//     where
//         V: Visitor<'de>,
//     {
//         match self.value {
//             Some(JsAny::Array(a)) => {
//                 let mut deserializer = JsArrayAccess::new(self.env, a);
//                 visitor.visit_seq(&mut deserializer)
//             },
//             _ => Err(serde::de::Error::invalid_type(
//                 Unexpected::UnitVariant,
//                 &"tuple variant",
//             )),
//         }
//     }

//     fn struct_variant<V>(self, _fields: &'detatic [&'detatic str], visitor: V,) -> Result<V::Value>
//     where
//         V: Visitor<'de>,
//     {
//         match self.value {
//             Some(JsAny::Object(o)) => {
//                 let mut deserializer = JsObjectAccess::new(self.env, o)?;
//                 visitor.visit_map(&mut deserializer)
//             },
//             _ => Err(serde::de::Error::invalid_type(
//                 Unexpected::UnitVariant,
//                 &"struct variant",
//             )),
//         }
//     }
// }
