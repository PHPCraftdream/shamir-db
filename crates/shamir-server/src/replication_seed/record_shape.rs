use std::collections::BTreeSet;
use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use shamir_types::core::interner::InternerKey;

/// Structural preflight prevents InnerValue's map decoder from silently
/// collapsing duplicate field IDs. Depth is bounded by the caller's decoder.
pub(crate) struct RecordShape;

impl<'de> Deserialize<'de> for RecordShape {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_any(ShapeVisitor)
    }
}

struct ShapeVisitor;

impl<'de> Visitor<'de> for ShapeVisitor {
    type Value = RecordShape;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("bounded record value with unique interned map keys")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(RecordShape)
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(RecordShape)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(RecordShape)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(RecordShape)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(RecordShape)
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(RecordShape)
    }
    fn visit_bytes<E: de::Error>(self, _: &[u8]) -> Result<Self::Value, E> {
        Ok(RecordShape)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut count = 0usize;
        while sequence.next_element::<RecordShape>()?.is_some() {
            count = count
                .checked_add(1)
                .ok_or_else(|| de::Error::custom("container overflow"))?;
            if count > 65536 {
                return Err(de::Error::custom("record container exceeds bounds"));
            }
        }
        Ok(RecordShape)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<InternerKey>()? {
            if !keys.insert(key.id()) {
                return Err(de::Error::custom("duplicate record field id"));
            }
            if keys.len() > 65536 {
                return Err(de::Error::custom("record container exceeds bounds"));
            }
            map.next_value::<RecordShape>()?;
        }
        Ok(RecordShape)
    }
}
