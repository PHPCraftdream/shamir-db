//! Owned deserialization of buffered query values without a wire-format round trip.

use serde::de::value::{Error, MapAccessDeserializer, MapDeserializer, SeqDeserializer};
use serde::de::{Deserializer, IntoDeserializer, Visitor};
use shamir_types::types::value::{QueryValue, Value};

pub(super) struct QueryValueDeserializer(pub(super) QueryValue);

impl<'de> IntoDeserializer<'de, Error> for QueryValueDeserializer {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

fn visit_sequence<'de, V: Visitor<'de>>(
    visitor: V,
    items: impl IntoIterator<Item = QueryValue>,
) -> Result<V::Value, Error> {
    let mut seq = SeqDeserializer::<_, Error>::new(items.into_iter().map(QueryValueDeserializer));
    let value = visitor.visit_seq(&mut seq)?;
    // Same as rmp_serde: trailing elements the visitor did not read are an error.
    seq.end()?;
    Ok(value)
}

impl QueryValueDeserializer {
    /// rmp_serde presented MessagePack binary as a sequence of bytes whenever a
    /// sequence-shaped type was requested; every other value goes through `any`.
    fn deserialize_sequence_like<'de, V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self.0 {
            Value::Bin(bytes) => visit_sequence(
                visitor,
                bytes.into_iter().map(|byte| Value::Int(i64::from(byte))),
            ),
            other => QueryValueDeserializer(other).deserialize_any(visitor),
        }
    }
}

impl<'de> Deserializer<'de> for QueryValueDeserializer {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::Null => visitor.visit_unit(),
            Value::Bool(v) => visitor.visit_bool(v),
            Value::Int(v) => visitor.visit_i64(v),
            Value::F64(v) => visitor.visit_f64(v),
            Value::Dec(v) => visitor.visit_string(v.to_string()),
            Value::Big(v) => {
                visitor.visit_u64(v.to_string().parse().map_err(serde::de::Error::custom)?)
            }
            Value::Str(v) => visitor.visit_string(v),
            Value::Bin(v) => visitor.visit_byte_buf(v),
            Value::List(v) => visit_sequence(visitor, v),
            Value::Set(v) => visit_sequence(visitor, v),
            Value::Map(v) => {
                let mut map = MapDeserializer::<'de, _, Error>::new(
                    v.into_iter().map(|(k, v)| (k, QueryValueDeserializer(v))),
                );
                let value = visitor.visit_map(&mut map)?;
                // Same as rmp_serde: a visitor that stops early must not drop entries silently.
                map.end()?;
                Ok(value)
            }
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::Null => visitor.visit_none(),
            value => visitor.visit_some(QueryValueDeserializer(value)),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self.0 {
            Value::Str(value) => visitor.visit_enum(value.into_deserializer()),
            // rmp_serde decoded any non-map input as a unit-variant identifier, so a
            // variant index or a byte-string name stayed accepted.
            Value::Bin(bytes) => {
                let name = String::from_utf8(bytes)
                    .map_err(|_| serde::de::Error::custom("enum identifier is not valid UTF-8"))?;
                visitor.visit_enum(name.into_deserializer())
            }
            Value::Int(index) => {
                let index = u32::try_from(index)
                    .map_err(|_| serde::de::Error::custom("enum variant index out of range"))?;
                visitor.visit_enum(index.into_deserializer())
            }
            Value::Map(value) if value.len() == 1 => {
                visitor.visit_enum(MapAccessDeserializer::new(MapDeserializer::new(
                    value
                        .into_iter()
                        .map(|(k, v)| (k, QueryValueDeserializer(v))),
                )))
            }
            _ => Err(serde::de::Error::custom(
                "expected a string, a variant index, bytes or a one-entry map for enum",
            )),
        }
    }

    fn is_human_readable(&self) -> bool {
        false
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_sequence_like(visitor)
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_sequence_like(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_sequence_like(visitor)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_sequence_like(visitor)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct map identifier ignored_any
    }
}
