//! MessagePack codec with on-the-fly key interning
//!
//! This module provides MessagePack encoding/decoding directly to/from InnerValue
//! without using UserValue (which is deprecated and for tests only).

use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::common::new_map;
use crate::types::value::{InnerValue, Value};
use rmpv::Value as RmpvValue;

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

/// Encodes InnerValue to MessagePack bytes, de-interning keys
///
/// This function:
/// 1. Converts InnerValue (InternedKey keys) to rmpv::Value
/// 2. Encodes rmpv::Value to MessagePack bytes
pub fn inner_to_msgpack(interner: &Interner, value: &InnerValue) -> Result<Vec<u8>, CodecError> {
    let rmpv_value = inner_to_rmpv_value(value, interner);

    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &rmpv_value)
        .map_err(|e| CodecError::Encode(format!("MessagePack encode error: {}", e)))?;

    Ok(buf)
}

/// Converts rmpv::Value to InnerValue, interning all string keys
fn rmpv_value_to_inner(
    rmpv_value: &RmpvValue,
    interner: &Interner,
) -> Result<InnerValue, CodecError> {
    match rmpv_value {
        RmpvValue::Nil => Ok(InnerValue::Nil),
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
                // Convert key
                let key_str = match rmpv_value_to_inner(key_val, interner)? {
                    InnerValue::Str(s) => s,
                    _ => return Err(CodecError::Decode("Map keys must be strings".to_string())),
                };

                // Intern the key
                let interned_key = interner
                    .touch_ind(&key_str)
                    .map_err(|e| CodecError::Decode(format!("Failed to intern key: {}", e)))?
                    .key()
                    .clone();

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

/// Converts InnerValue to rmpv::Value, de-interning all keys
fn inner_to_rmpv_value(value: &InnerValue, interner: &Interner) -> RmpvValue {
    match value {
        Value::Nil => RmpvValue::Nil,
        Value::Bool(b) => RmpvValue::Boolean(*b),
        Value::Int(i) => RmpvValue::Integer((*i).into()),
        Value::F64(f) => RmpvValue::F64(*f),
        Value::Dec(d) => RmpvValue::String(d.to_string().into()),
        Value::Big(b) => RmpvValue::String(b.to_string().into()),
        Value::Str(s) => RmpvValue::String(s.clone().into()),
        Value::Bin(b) => RmpvValue::Binary(b.clone()),
        Value::List(l) => {
            RmpvValue::Array(l.iter().map(|v| inner_to_rmpv_value(v, interner)).collect())
        }
        Value::Set(s) => {
            // Sets become arrays in MessagePack (similar to JSON)
            RmpvValue::Array(s.iter().map(|v| inner_to_rmpv_value(v, interner)).collect())
        }
        Value::Map(m) => {
            let mut map = Vec::new();
            for (interned_key, val) in m {
                let key_str = interner
                    .get_str(interned_key)
                    .expect("Interned key not found in interner")
                    .as_ref()
                    .to_string();
                let key = RmpvValue::String(key_str.into());
                let rmpv_val = inner_to_rmpv_value(val, interner);
                map.push((key, rmpv_val));
            }
            RmpvValue::Map(map)
        }
    }
}
