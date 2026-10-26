//! Guest-side `Value` type — wire-compatible with `QueryValue` via msgpack.
//!
//! This is a **minimal mirror** of the host `Value<String>` with identical
//! MessagePack serialization so the host decodes guest output byte-for-byte.
//! The host type lives in `shamir-types` (a dev-dependency only — it never
//! enters the guest WASM binary).
//!
//! **Deliberately omitted variants** (the guest sees them on the wire as
//! simpler types — see conformance tests in `tests/value_tests.rs`):
//! - `Dec` / `Big` — serialized by the host as strings → decoded here as `Str`.
//! - `Set` — serialized by the host as a sequence → decoded here as `List`.
//!
//! `Map` uses `Vec<(String, Value)>` instead of `IndexMap` to avoid the
//! `indexmap` dependency. Serialization order is preserved; deserialization
//! order matches.

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use std::fmt;

const MAX_NESTING: usize = 32;

// Planning estimate only, not measured frame size or a stack-safety guarantee:
// on wasm32 the Value enum is expected to be about 24 bytes (String/Vec are
// 12-byte headers), with ValueSeed/ValueVisitor, SeqAccess/MapAccess state,
// deserializer references, child temporaries, error construction, and serde
// frames also live during recursive descent. Allowing a conservative 4 KiB per
// active container gives 32 * 4096 = 128 KiB, before caller headroom. No
// repository wasm32 stack-size pin was found; the toolchain default is not a
// repository promise. Plan for at least 128 KiB plus caller budget; this is not
// target-specific proof of actual stack use.

/// A ShamirDB value.
///
/// Wire-compatible with `shamir_types::QueryValue` via MessagePack for the
/// supported variants (Null, Bool, Int, F64, Str, Bin, List, Map).
/// Deserialization accepts at most 32 nested list/map containers. This is a
/// codec nesting limit; it is not a proof about guest stack usage.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    F64(f64),
    Str(String),
    Bin(Vec<u8>),
    List(Vec<Value>),
    Map(Vec<(String, Value)>),
}

impl Serialize for Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Value::Null => serializer.serialize_unit(),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Int(i) => serializer.serialize_i64(*i),
            Value::F64(f) => serializer.serialize_f64(*f),
            Value::Str(s) => serializer.serialize_str(s),
            Value::Bin(b) => serializer.serialize_bytes(b),
            Value::List(l) => {
                let mut seq = serializer.serialize_seq(Some(l.len()))?;
                for element in l {
                    seq.serialize_element(element)?;
                }
                seq.end()
            }
            Value::Map(m) => {
                let mut map = serializer.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

struct ValueSeed {
    depth: usize,
}

impl<'de> de::DeserializeSeed<'de> for ValueSeed {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(ValueVisitor { depth: self.depth })
    }
}

struct ValueVisitor {
    depth: usize,
}

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a ShamirDB value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(Value::Int(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        if v <= i64::MAX as u64 {
            Ok(Value::Int(v as i64))
        } else {
            Ok(Value::Str(v.to_string()))
        }
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        Ok(Value::F64(v))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(Value::Str(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
        Ok(Value::Str(v))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
        Ok(Value::Bin(v.to_vec()))
    }

    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
        Ok(Value::Bin(v))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let depth = self.depth + 1;
        if depth > MAX_NESTING {
            return Err(de::Error::custom("maximum Value nesting depth exceeded"));
        }
        let mut list = Vec::new();
        while let Some(elem) = seq.next_element_seed(ValueSeed { depth })? {
            list.push(elem);
        }
        Ok(Value::List(list))
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let depth = self.depth + 1;
        if depth > MAX_NESTING {
            return Err(de::Error::custom("maximum Value nesting depth exceeded"));
        }
        let mut entries = Vec::new();
        while let Some(key) = map.next_key()? {
            let value = map.next_value_seed(ValueSeed { depth })?;
            entries.push((key, value));
        }
        Ok(Value::Map(entries))
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        ValueSeed { depth: 0 }.deserialize(deserializer)
    }
}
