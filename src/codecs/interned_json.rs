//! JSON codec with on-the-fly key interning
//!
//! This module provides JSON encoding/decoding directly to/from InnerValue
//! without using UserValue (which is deprecated and for tests only).

use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::value::{InnerValue, Value};
use crate::types::common::new_map;
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
fn json_value_to_inner(json_value: &json::Value, interner: &Interner) -> Result<InnerValue, CodecError> {
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
                let interned_key = interner.touch_ind(key_str)
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
            json::Value::Array(b.iter().map(|&byte| json::Value::Number(byte.into())).collect())
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
                let key_str = interner.get_str(interned_key)
                    .expect("Interned key not found in interner")
                    .as_ref()
                    .to_string();
                obj.insert(key_str, inner_to_json_value(val, interner));
            }
            json::Value::Object(obj)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::new_map;

    #[test]
    fn test_json_to_inner_simple() {
        let interner = Interner::new();
        let json = r#"{"name":"Alice","age":30}"#;

        let inner = json_to_inner(&interner, json.as_bytes()).unwrap();

        match inner {
            InnerValue::Map(m) => {
                assert_eq!(m.len(), 2);

                // Check name
                let name_key = interner.touch_ind("name").unwrap().key().clone();
                assert!(m.contains_key(&name_key));
                assert_eq!(m.get(&name_key), Some(&InnerValue::Str("Alice".to_string())));

                // Check age
                let age_key = interner.touch_ind("age").unwrap().key().clone();
                assert!(m.contains_key(&age_key));
                assert_eq!(m.get(&age_key), Some(&InnerValue::Int(30)));
            }
            _ => panic!("Expected Map"),
        }
    }

    #[test]
    fn test_inner_to_json_simple() {
        let interner = Interner::new();

        let name_key = interner.touch_ind("name").unwrap().key().clone();
        let age_key = interner.touch_ind("age").unwrap().key().clone();

        let mut map = new_map();
        map.insert(name_key, InnerValue::Str("Alice".to_string()));
        map.insert(age_key, InnerValue::Int(30));

        let inner = InnerValue::Map(map);
        let json = inner_to_json(&interner, &inner).unwrap();
        let json_str = String::from_utf8(json).unwrap();

        assert!(json_str.contains("\"name\":\"Alice\"") || json_str.contains("\"name\": \"Alice\""));
        assert!(json_str.contains("\"age\":30") || json_str.contains("\"age\": 30"));
    }

    #[test]
    fn test_roundtrip_json() {
        let interner = Interner::new();
        let original_json = r#"{
            "name": "Alice",
            "age": 30,
            "active": true,
            "tags": ["rust", "db"]
        }"#;

        let inner = json_to_inner(&interner, original_json.as_bytes()).unwrap();
        let result_json = inner_to_json(&interner, &inner).unwrap();
        let result_str = String::from_utf8(result_json).unwrap();

        // Parse both and compare semantically (ignore whitespace)
        let original: serde_json::Value = serde_json::from_str(original_json).unwrap();
        let result: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(original, result);
    }

    #[test]
    fn test_nested_structures() {
        let interner = Interner::new();
        let json = r#"{
            "user": {
                "name": "Bob",
                "prefs": {"theme": "dark"}
            },
            "items": [1, 2, 3]
        }"#;

        let inner = json_to_inner(&interner, json.as_bytes()).unwrap();
        let result = inner_to_json(&interner, &inner).unwrap();

        let original_val: serde_json::Value = serde_json::from_str(json).unwrap();
        let result_val: serde_json::Value = serde_json::from_slice(&result).unwrap();

        assert_eq!(original_val, result_val);
    }

    #[test]
    fn test_interning_reuse() {
        let interner = Interner::new();
        let json = r#"{"key":"value","nested":{"key":"other"}}"#;

        json_to_inner(&interner, json.as_bytes()).unwrap();

        // "key" should be interned only once
        let key_interned = interner.touch_ind("key").unwrap();
        assert!(!key_interned.is_new(), "Key should already be interned");
    }

    #[test]
    fn test_all_json_types() {
        let interner = Interner::new();
        let json = r#"{
            "null_val": null,
            "bool_true": true,
            "bool_false": false,
            "int_val": -42,
            "float_val": 3.14,
            "string_val": "hello",
            "array_val": [1, 2, 3],
            "nested_obj": {"key": "value"}
        }"#;

        let inner = json_to_inner(&interner, json.as_bytes()).unwrap();

        match inner {
            InnerValue::Map(m) => {
                assert_eq!(m.len(), 8);
            }
            _ => panic!("Expected Map"),
        }
    }

    #[test]
    fn test_error_invalid_json() {
        let interner = Interner::new();
        let invalid_json = b"{invalid json}";

        let result = json_to_inner(&interner, invalid_json);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_value() {
        let interner = Interner::new();
        let json = r#"{}"#;

        let inner = json_to_inner(&interner, json.as_bytes()).unwrap();
        let result = inner_to_json(&interner, &inner).unwrap();

        assert_eq!(result, b"{}");
    }

    #[test]
    fn test_binary_data_as_array() {
        let interner = Interner::new();
        let inner = InnerValue::Bin(vec![1, 2, 3, 4, 5]);
        let json = inner_to_json(&interner, &inner).unwrap();

        // Binary should be encoded as array of numbers
        let json_val: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert!(json_val.is_array());
    }

    #[test]
    fn test_special_numbers() {
        let interner = Interner::new();

        // Test large unsigned integer
        let large_uint: u64 = 9007199254740992; // 2^53
        let json = format!(r#"{{"large": {}}}"#, large_uint);
        let inner = json_to_inner(&interner, json.as_bytes()).unwrap();

        match inner {
            InnerValue::Map(m) => {
                let key = interner.touch_ind("large").unwrap().key().clone();
                if let Some(InnerValue::Int(i)) = m.get(&key) {
                    assert_eq!(*i as u64, large_uint);
                } else {
                    panic!("Expected Int value");
                }
            }
            _ => panic!("Expected Map"),
        }
    }
}
