//! MessagePack codec with on-the-fly key interning
//!
//! This module provides MessagePack encoding/decoding directly to/from InnerValue
//! without using UserValue (which is deprecated and for tests only).

use crate::codecs::interned::common::intern_string_key;
use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::common::new_map;
use crate::types::value::{InnerValue, Value};
use rmpv::Value as RmpvValue;
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

/// Decodes MessagePack bytes to InnerValue, interning string keys
///
/// This function:
/// 1. Parses MessagePack bytes into rmpv::Value
/// 2. Converts to InnerValue, interning all string keys
/// 3. Returns InnerValue (InternedKey keys)
pub fn msgpack_to_inner(interner: &Interner, bytes: &[u8]) -> Result<InnerValue, CodecError> {
    let rmpv_value: RmpvValue = rmpv::decode::read_value(&mut &*bytes)
        .map_err(|e| CodecError::Decode(format!("MessagePack decode error: {}", e)))?;

    rmpv_value_to_inner(&rmpv_value, interner)
}

/// Encodes InnerValue to MessagePack bytes, de-interning keys.
///
/// Streams directly into the output buffer via `rmp_serde::to_vec`
/// over an `InternedRef` wrapper. No intermediate `rmpv::Value`
/// tree is built — map keys are de-interned to `&str` and written
/// in-place when each Map entry is serialised.
pub fn inner_to_msgpack(interner: &Interner, value: &InnerValue) -> Result<Vec<u8>, CodecError> {
    rmp_serde::to_vec(&InternedRef { value, interner })
        .map_err(|e| CodecError::Encode(format!("MessagePack encode error: {}", e)))
}

/// Borrowed view of an `InnerValue` paired with the `Interner` used
/// to resolve its map keys. Implements `Serialize` so `rmp_serde` (or
/// any other serde format) can write it without an intermediate value
/// tree.
struct InternedRef<'a> {
    value: &'a InnerValue,
    interner: &'a Interner,
}

impl Serialize for InternedRef<'_> {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match self.value {
            Value::Null => ser.serialize_unit(),
            Value::Bool(b) => ser.serialize_bool(*b),
            Value::Int(i) => ser.serialize_i64(*i),
            Value::F64(f) => ser.serialize_f64(*f),
            Value::Dec(d) => ser.serialize_str(&d.to_string()),
            Value::Big(b) => ser.serialize_str(&b.to_string()),
            Value::Str(s) => ser.serialize_str(s),
            Value::Bin(b) => ser.serialize_bytes(b),
            Value::List(l) => {
                let mut seq = ser.serialize_seq(Some(l.len()))?;
                for el in l {
                    seq.serialize_element(&InternedRef {
                        value: el,
                        interner: self.interner,
                    })?;
                }
                seq.end()
            }
            Value::Set(s) => {
                let mut seq = ser.serialize_seq(Some(s.len()))?;
                for el in s {
                    seq.serialize_element(&InternedRef {
                        value: el,
                        interner: self.interner,
                    })?;
                }
                seq.end()
            }
            Value::Map(m) => {
                let mut map = ser.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    // Borrow the interned key string without allocating
                    // a `UserKey(String)` per entry (hunt #7). The
                    // `with_str` closure scope covers the
                    // `serialize_entry` call so the `&str` stays live.
                    let r: Result<(), S::Error> = self
                        .interner
                        .with_str(k, |key_str| {
                            map.serialize_entry(
                                key_str,
                                &InternedRef {
                                    value: v,
                                    interner: self.interner,
                                },
                            )
                        })
                        .ok_or_else(|| {
                            serde::ser::Error::custom(format!(
                                "Interned key not found in interner: {:?}",
                                k
                            ))
                        })?;
                    r?;
                }
                map.end()
            }
        }
    }
}

/// Converts rmpv::Value to InnerValue, interning all string keys
fn rmpv_value_to_inner(
    rmpv_value: &RmpvValue,
    interner: &Interner,
) -> Result<InnerValue, CodecError> {
    match rmpv_value {
        RmpvValue::Nil => Ok(InnerValue::Null),
        RmpvValue::Boolean(b) => Ok(InnerValue::Bool(*b)),
        RmpvValue::Integer(n) => {
            if n.is_u64() {
                let u = n.as_u64().unwrap();
                if u <= i64::MAX as u64 {
                    Ok(InnerValue::Int(u as i64))
                } else {
                    Ok(InnerValue::Str(u.to_string()))
                }
            } else if n.is_i64() {
                Ok(InnerValue::Int(n.as_i64().unwrap()))
            } else {
                // Very large integers as string
                Ok(InnerValue::Str(n.to_string()))
            }
        }
        RmpvValue::F64(f) => Ok(InnerValue::F64(*f)),
        RmpvValue::F32(f) => Ok(InnerValue::F64(*f as f64)),
        RmpvValue::String(s) => {
            let s_str = s
                .as_str()
                .ok_or_else(|| CodecError::Decode("Invalid UTF-8 in string".to_string()))?;
            Ok(InnerValue::Str(s_str.to_string()))
        }
        RmpvValue::Binary(b) => Ok(InnerValue::Bin(b.clone())),
        RmpvValue::Array(arr) => {
            let converted: Result<Vec<InnerValue>, CodecError> = arr
                .iter()
                .map(|v| rmpv_value_to_inner(v, interner))
                .collect();
            Ok(InnerValue::List(converted?))
        }
        RmpvValue::Map(map) => {
            let mut converted = new_map();
            for (key_val, val) in map {
                // Intern map keys directly from the borrowed `&str`
                // owned by `rmpv::Value::String` — no transient
                // `String` allocation, no intermediate
                // `InnerValue::Str` build-then-discard per key
                // (mirror of the encode-side `with_str` borrow win).
                let key_str = match key_val {
                    RmpvValue::String(s) => s.as_str().ok_or_else(|| {
                        CodecError::Decode("Invalid UTF-8 in map key".to_string())
                    })?,
                    _ => return Err(CodecError::Decode("Map keys must be strings".to_string())),
                };

                // Intern the key from the borrowed slice.
                let interned_key = intern_string_key(interner, key_str)?;

                // Convert value
                let converted_val = rmpv_value_to_inner(val, interner)?;
                converted.insert(interned_key, converted_val);
            }
            Ok(InnerValue::Map(converted))
        }
        RmpvValue::Ext(_type, data) => {
            // Extension types - store as binary for now
            Ok(InnerValue::Bin(data.clone()))
        }
    }
}
