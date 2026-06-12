//! JSON codec with on-the-fly key interning
//!
//! This module provides JSON encoding/decoding directly to/from InnerValue
//! without using UserValue (which is deprecated and for tests only).

use crate::codecs::interned::common::intern_string_key;
use crate::codecs::CodecError;
use crate::core::interner::{Interner, InternerKey, UserKey};
use crate::types::common::new_map;
use crate::types::value::{InnerValue, Value};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json as json;

/// Decodes JSON bytes to InnerValue, interning string keys
///
/// This function:
/// 1. Parses JSON bytes into serde_json::Value
/// 2. Converts to InnerValue, interning all string keys
/// 3. Returns InnerValue (InternedKey keys)
pub fn json_to_inner(interner: &Interner, bytes: &[u8]) -> Result<InnerValue, CodecError> {
    let json_value: json::Value = json::from_slice(bytes)
        .map_err(|e| CodecError::Decode(format!("JSON decode error: {}", e)))?;

    json_value_to_inner(&json_value, interner)
}

/// Encodes InnerValue to JSON bytes, de-interning keys.
///
/// Streams the JSON straight into the output buffer via
/// `serde_json::to_vec` over an `InternedRef` wrapper — no
/// intermediate `json::Value` tree. Map keys are de-interned to
/// `&str` and written in place when each entry is serialised.
pub fn inner_to_json(interner: &Interner, value: &InnerValue) -> Result<Vec<u8>, CodecError> {
    json::to_vec(&InternedRef { value, interner })
        .map_err(|e| CodecError::Encode(format!("JSON encode error: {}", e)))
}

/// Borrowed view of an `InnerValue` paired with the `Interner` used to
/// resolve its map keys. Implements `Serialize` so any serde format
/// (here `serde_json`) can write it without an intermediate value tree.
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
            Value::F64(f) => {
                if f.is_finite() {
                    ser.serialize_f64(*f)
                } else {
                    // serde_json refuses non-finite floats; the old
                    // path converted them via `f.to_string()`.
                    ser.serialize_str(&f.to_string())
                }
            }
            Value::Dec(d) => ser.serialize_str(&d.to_string()),
            Value::Big(b) => ser.serialize_str(&b.to_string()),
            Value::Str(s) => ser.serialize_str(s),
            Value::Bin(b) => {
                // JSON has no binary type — the old path emitted an
                // array of byte numbers. Preserve that shape.
                let mut seq = ser.serialize_seq(Some(b.len()))?;
                for byte in b {
                    seq.serialize_element(byte)?;
                }
                seq.end()
            }
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
                for (interned_key, val) in m {
                    // Borrow the interned key string without allocating
                    // a `UserKey(String)` per entry (hunt #7). The
                    // `with_str` closure scope covers the
                    // `serialize_entry` call so the `&str` stays live.
                    let r: Result<(), S::Error> = self
                        .interner
                        .with_str(interned_key, |k| {
                            map.serialize_entry(
                                k,
                                &InternedRef {
                                    value: val,
                                    interner: self.interner,
                                },
                            )
                        })
                        .ok_or_else(|| {
                            serde::ser::Error::custom(format!(
                                "Interned key not found in interner: {:?}",
                                interned_key
                            ))
                        })?;
                    r?;
                }
                map.end()
            }
        }
    }
}

/// Converts serde_json::Value to InnerValue using a custom key-interning
/// function.
///
/// This allows callers to plug in a `LayeredInterner` or any other
/// interning strategy without depending on the `Interner` type directly.
pub fn json_value_to_inner_with<F>(
    json_value: &json::Value,
    intern_key: &F,
) -> Result<InnerValue, CodecError>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    match json_value {
        json::Value::Null => Ok(InnerValue::Null),
        json::Value::Bool(b) => Ok(InnerValue::Bool(*b)),
        json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(InnerValue::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(InnerValue::F64(f))
            } else if let Some(u) = n.as_u64() {
                // Large unsigned integers - convert to Int if fits, otherwise store as string
                if u <= i64::MAX as u64 {
                    Ok(InnerValue::Int(u as i64))
                } else {
                    Ok(InnerValue::Str(u.to_string()))
                }
            } else {
                Ok(InnerValue::Str(n.to_string()))
            }
        }
        json::Value::String(s) => Ok(InnerValue::Str(s.clone())),
        json::Value::Array(arr) => {
            let converted: Result<Vec<InnerValue>, CodecError> = arr
                .iter()
                .map(|v| json_value_to_inner_with(v, intern_key))
                .collect();
            Ok(InnerValue::List(converted?))
        }
        json::Value::Object(obj) => {
            let mut converted = new_map();
            for (key_str, val) in obj {
                let interned_key = intern_key(key_str)?;
                let converted_val = json_value_to_inner_with(val, intern_key)?;
                converted.insert(interned_key, converted_val);
            }
            Ok(InnerValue::Map(converted))
        }
    }
}

/// Converts serde_json::Value to InnerValue, interning all string keys
pub fn json_value_to_inner(
    json_value: &json::Value,
    interner: &Interner,
) -> Result<InnerValue, CodecError> {
    json_value_to_inner_with(json_value, &|key| intern_string_key(interner, key))
}

/// Converts a [`QueryValue`] (string-keyed) to [`InnerValue`] (interned keys).
///
/// This is the key function for the zero-JSON write path: once user data
/// is deserialized as `QueryValue` (format-agnostic), this pass interns
/// the map keys to produce the engine-native representation.
pub fn query_value_to_inner(
    qv: &crate::types::value::QueryValue,
    interner: &Interner,
) -> Result<InnerValue, CodecError> {
    query_value_to_inner_with(qv, &|key| intern_string_key(interner, key))
}

/// Converts a [`QueryValue`] to [`InnerValue`] using a custom interning function.
pub fn query_value_to_inner_with<F>(
    qv: &crate::types::value::QueryValue,
    intern_key: &F,
) -> Result<InnerValue, CodecError>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    use crate::types::value::Value;
    match qv {
        Value::Null => Ok(InnerValue::Null),
        Value::Bool(b) => Ok(InnerValue::Bool(*b)),
        Value::Int(i) => Ok(InnerValue::Int(*i)),
        Value::F64(f) => Ok(InnerValue::F64(*f)),
        Value::Dec(d) => Ok(InnerValue::Dec(*d)),
        Value::Big(b) => Ok(InnerValue::Big(b.clone())),
        Value::Str(s) => Ok(InnerValue::Str(s.clone())),
        Value::Bin(b) => Ok(InnerValue::Bin(b.clone())),
        Value::List(l) => {
            let converted: Result<Vec<InnerValue>, CodecError> = l
                .iter()
                .map(|v| query_value_to_inner_with(v, intern_key))
                .collect();
            Ok(InnerValue::List(converted?))
        }
        Value::Set(s) => {
            let converted: Result<crate::types::common::TSet<InnerValue>, CodecError> = s
                .iter()
                .map(|v| query_value_to_inner_with(v, intern_key))
                .collect();
            Ok(InnerValue::Set(converted?))
        }
        Value::Map(m) => {
            let mut converted = crate::types::common::new_map();
            for (key_str, val) in m {
                let interned_key = intern_key(key_str)?;
                let converted_val = query_value_to_inner_with(val, intern_key)?;
                converted.insert(interned_key, converted_val);
            }
            Ok(InnerValue::Map(converted))
        }
    }
}

/// Converts [`InnerValue`] (interned keys) to [`QueryValue`] (string keys),
/// de-interning map keys via a single reverse-snapshot acquisition.
///
/// Mirrors the semantics of [`inner_to_json_value`] — same key-resolution
/// behaviour and same error handling for missing intern keys — but builds
/// the allocation-light `QueryValue` tree instead of a `serde_json::Value`.
pub fn inner_value_to_query_value(
    value: &InnerValue,
    interner: &Interner,
) -> Result<crate::types::value::QueryValue, CodecError> {
    let rev = interner.reverse_snapshot();
    inner_value_to_query_value_with_rev(value, rev.as_slice())
}

fn inner_value_to_query_value_with_rev(
    value: &InnerValue,
    rev: &[Option<UserKey>],
) -> Result<crate::types::value::QueryValue, CodecError> {
    use crate::types::common::{new_map_wc, TSet};
    match value {
        Value::Null => Ok(crate::types::value::QueryValue::Null),
        Value::Bool(b) => Ok(crate::types::value::QueryValue::Bool(*b)),
        Value::Int(i) => Ok(crate::types::value::QueryValue::Int(*i)),
        Value::F64(f) => Ok(crate::types::value::QueryValue::F64(*f)),
        Value::Dec(d) => Ok(crate::types::value::QueryValue::Dec(*d)),
        Value::Big(b) => Ok(crate::types::value::QueryValue::Big(b.clone())),
        Value::Str(s) => Ok(crate::types::value::QueryValue::Str(s.clone())),
        Value::Bin(b) => Ok(crate::types::value::QueryValue::Bin(b.clone())),
        Value::List(l) => {
            let arr: Result<Vec<_>, _> = l
                .iter()
                .map(|v| inner_value_to_query_value_with_rev(v, rev))
                .collect();
            Ok(crate::types::value::QueryValue::List(arr?))
        }
        Value::Set(s) => {
            let converted: Result<TSet<crate::types::value::QueryValue>, _> = s
                .iter()
                .map(|v| inner_value_to_query_value_with_rev(v, rev))
                .collect();
            Ok(crate::types::value::QueryValue::Set(converted?))
        }
        Value::Map(m) => {
            let mut obj = new_map_wc(m.len());
            for (interned_key, val) in m {
                let idx = interned_key.id() as usize;
                let key_str = rev
                    .get(idx)
                    .and_then(|slot| slot.as_ref())
                    .map(|k| k.as_str().to_string())
                    .ok_or_else(|| {
                        CodecError::Decode(format!("Interned key not found: {:?}", interned_key))
                    })?;
                obj.insert(key_str, inner_value_to_query_value_with_rev(val, rev)?);
            }
            Ok(crate::types::value::QueryValue::Map(obj))
        }
    }
}

/// Converts InnerValue to serde_json::Value, de-interning all keys.
///
/// Hoists the interner's reverse-vec `ArcSwap` load to a single
/// acquisition for the entire walk, instead of paying one
/// `ArcSwap::load` + bounds check per map key as the recursion
/// descends (per-Put cost in the subscription filter hot path was
/// `O(fields)` ArcSwap loads — 50 for a 50-field record).
pub fn inner_to_json_value(
    value: &InnerValue,
    interner: &Interner,
) -> Result<json::Value, CodecError> {
    let rev = interner.reverse_snapshot();
    inner_to_json_value_with_rev(value, rev.as_slice())
}

fn inner_to_json_value_with_rev(
    value: &InnerValue,
    rev: &[Option<UserKey>],
) -> Result<json::Value, CodecError> {
    match value {
        Value::Null => Ok(json::Value::Null),
        Value::Bool(b) => Ok(json::Value::Bool(*b)),
        Value::Int(i) => Ok(json::Value::Number((*i).into())),
        Value::F64(f) => {
            if f.is_finite() {
                if let Some(n) = serde_json::Number::from_f64(*f) {
                    Ok(json::Value::Number(n))
                } else {
                    Ok(json::Value::String(f.to_string()))
                }
            } else {
                Ok(json::Value::String(f.to_string()))
            }
        }
        Value::Dec(d) => Ok(json::Value::String(d.to_string())),
        Value::Big(b) => Ok(json::Value::String(b.to_string())),
        Value::Str(s) => Ok(json::Value::String(s.clone())),
        Value::Bin(b) => Ok(json::Value::Array(
            b.iter()
                .map(|&byte| json::Value::Number(byte.into()))
                .collect(),
        )),
        Value::List(l) => {
            let arr: Result<Vec<_>, _> = l
                .iter()
                .map(|v| inner_to_json_value_with_rev(v, rev))
                .collect();
            Ok(json::Value::Array(arr?))
        }
        Value::Set(s) => {
            let arr: Result<Vec<_>, _> = s
                .iter()
                .map(|v| inner_to_json_value_with_rev(v, rev))
                .collect();
            Ok(json::Value::Array(arr?))
        }
        Value::Map(m) => {
            let mut obj = json::Map::new();
            for (interned_key, val) in m {
                let idx = interned_key.id() as usize;
                let key_str = rev
                    .get(idx)
                    .and_then(|slot| slot.as_ref())
                    .map(|k| k.as_str().to_string())
                    .ok_or_else(|| {
                        CodecError::Decode(format!("Interned key not found: {:?}", interned_key))
                    })?;
                obj.insert(key_str, inner_to_json_value_with_rev(val, rev)?);
            }
            Ok(json::Value::Object(obj))
        }
    }
}
