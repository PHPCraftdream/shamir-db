//! JSON codec with on-the-fly key interning
//!
//! This module provides JSON encoding/decoding directly to/from InnerValue
//! without using UserValue (which is deprecated and for tests only).

use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::common::new_map;
use crate::types::value::{InnerValue, Value};
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

/// Encodes InnerValue to JSON bytes, de-interning keys
///
/// This function:
/// 1. Converts InnerValue (InternedKey keys) to serde_json::Value
/// 2. Encodes serde_json::Value to JSON bytes
pub fn inner_to_json(interner: &Interner, value: &InnerValue) -> Result<Vec<u8>, CodecError> {
    let json_value = inner_to_json_value(value, interner);

    let bytes = json::to_vec(&json_value)
        .map_err(|e| CodecError::Encode(format!("JSON encode error: {}", e)))?;

    Ok(bytes)
}

/// Converts serde_json::Value to InnerValue, interning all string keys
fn json_value_to_inner(
    json_value: &json::Value,
    interner: &Interner,
) -> Result<InnerValue, CodecError> {
    match json_value {
        json::Value::Null => Ok(InnerValue::Nil),
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
                .map(|v| json_value_to_inner(v, interner))
                .collect();
            Ok(InnerValue::List(converted?))
        }
        json::Value::Object(obj) => {
            let mut converted = new_map();
            for (key_str, val) in obj {
                let interned_key = interner
                    .touch_ind(key_str)
                    .map_err(|e| CodecError::Decode(format!("Failed to intern key: {}", e)))?
                    .key()
                    .clone();
                let converted_val = json_value_to_inner(val, interner)?;
                converted.insert(interned_key, converted_val);
            }
            Ok(InnerValue::Map(converted))
        }
    }
}

/// Converts InnerValue to serde_json::Value, de-interning all keys
fn inner_to_json_value(value: &InnerValue, interner: &Interner) -> json::Value {
    match value {
        Value::Nil => json::Value::Null,
        Value::Bool(b) => json::Value::Bool(*b),
        Value::Int(i) => json::Value::Number((*i).into()),
        Value::F64(f) => {
            if f.is_finite() {
                if let Some(n) = serde_json::Number::from_f64(*f) {
                    json::Value::Number(n)
                } else {
                    json::Value::String(f.to_string())
                }
            } else {
                json::Value::String(f.to_string())
            }
        }
        Value::Dec(d) => json::Value::String(d.to_string()),
        Value::Big(b) => json::Value::String(b.to_string()),
        Value::Str(s) => json::Value::String(s.clone()),
        Value::Bin(b) => {
            // Binary data as base64 or array? JSON doesn't have binary type
            // Using array of numbers for simplicity
            json::Value::Array(
                b.iter()
                    .map(|&byte| json::Value::Number(byte.into()))
                    .collect(),
            )
        }
        Value::List(l) => {
            json::Value::Array(l.iter().map(|v| inner_to_json_value(v, interner)).collect())
        }
        Value::Set(s) => {
            // Sets become arrays in JSON, but we use "set:" prefix to distinguish them
            json::Value::Array(s.iter().map(|v| inner_to_json_value(v, interner)).collect())
        }
        Value::Map(m) => {
            let mut obj = json::Map::new();
            for (interned_key, val) in m {
                let key_str = interner
                    .get_str(interned_key)
                    .expect("Interned key not found in interner")
                    .as_ref()
                    .to_string();
                obj.insert(key_str, inner_to_json_value(val, interner));
            }
            json::Value::Object(obj)
        }
    }
}
