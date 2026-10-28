//! Streaming raw-wire preflight for capped request collections. Only known
//! request/batch/op contexts are traversed; other values use `IgnoredAny`.
//! This MUST run on untrusted request bytes before typed decoding: nested
//! `BatchOp` decoding buffers through `QueryValue`, which can collapse duplicate
//! map keys before `decode_caps` sees their original wire-entry count.
use crate::batch::batch_limits::BatchLimits;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserializer;
use std::fmt;

const MAX_ALIAS_ENTRIES: usize = BatchLimits::CEILING.max_queries;

#[derive(Clone, Copy)]
enum Shape {
    Request,
    Batch,
    Op,
}
struct Walk(Shape);
impl<'de> DeserializeSeed<'de> for Walk {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(MapVisitor(self.0))
    }
}
struct MapVisitor(Shape);
impl<'de> Visitor<'de> for MapVisitor {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a request structure map")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            match self.0 {
                Shape::Request if key == "batch" => map.next_value_seed(Walk(Shape::Batch))?,
                Shape::Batch if key == "queries" => map.next_value_seed(CappedMap(true))?,
                Shape::Batch if key == "interner_epochs" => {
                    map.next_value_seed(CappedMap(false))?
                }
                Shape::Batch if key == "return_only" => map.next_value_seed(CappedSeq)?,
                Shape::Op if matches!(key.as_str(), "batch" | "for_each") => {
                    map.next_value_seed(Walk(Shape::Batch))?
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}
struct CappedMap(bool);
impl<'de> DeserializeSeed<'de> for CappedMap {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(CappedMapVisitor(self.0))
    }
}
struct CappedMapVisitor(bool);
impl<'de> Visitor<'de> for CappedMapVisitor {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a capped map")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<(), A::Error> {
        let mut n = 0;
        while a.next_key::<IgnoredAny>()?.is_some() {
            n += 1;
            if n > MAX_ALIAS_ENTRIES {
                return Err(serde::de::Error::custom("decode cap exceeded: alias map"));
            }
            if self.0 {
                a.next_value_seed(Walk(Shape::Op))?;
            } else {
                a.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}
struct CappedSeq;
impl<'de> DeserializeSeed<'de> for CappedSeq {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_option(SeqOptionVisitor)
    }
}
struct SeqOptionVisitor;
impl<'de> Visitor<'de> for SeqOptionVisitor {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("null or a capped sequence")
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(SeqVisitor)
    }
}
struct SeqVisitor;
impl<'de> Visitor<'de> for SeqVisitor {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a capped sequence")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<(), A::Error> {
        let mut n = 0;
        while n <= MAX_ALIAS_ENTRIES && a.next_element::<IgnoredAny>()?.is_some() {
            n += 1;
        }
        if n > MAX_ALIAS_ENTRIES {
            return Err(serde::de::Error::custom(
                "decode cap exceeded: alias sequence",
            ));
        }
        Ok(())
    }
}
/// Validate capped collections directly from raw DbRequest MessagePack bytes.
pub fn validate_request_alias_caps(bytes: &[u8]) -> Result<(), rmp_serde::decode::Error> {
    let mut de = rmp_serde::Deserializer::new(bytes);
    Walk(Shape::Request).deserialize(&mut de)?;
    Ok(())
}
