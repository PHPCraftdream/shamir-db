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
            Value::Map(value) if value.len() == 1 => {
                visitor.visit_enum(MapAccessDeserializer::new(MapDeserializer::new(
                    value
                        .into_iter()
                        .map(|(k, v)| (k, QueryValueDeserializer(v))),
                )))
            }
            _ => Err(serde::de::Error::custom(
                "expected a string or a one-entry map for enum",
            )),
        }
    }

    fn is_human_readable(&self) -> bool {
        false
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}
