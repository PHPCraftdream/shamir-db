//! MessagePack codec with on-the-fly key interning
//!
//! This module provides MessagePack encoding/decoding directly to/from InnerValue
//! without using UserValue (which is deprecated and for tests only).

use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::value::{InnerValue, Value};
use crate::types::common::new_map;
use rmpv::{Value as RmpvValue};

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
fn rmpv_value_to_inner(rmpv_value: &RmpvValue, interner: &Interner) -> Result<InnerValue, CodecError> {
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
            let s_str = s.as_str().ok_or_else(|| CodecError::Decode("Invalid UTF-8 in string".to_string()))?;
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
                let interned_key = interner.touch_ind(&key_str)
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
                let key_str = interner.get_str(interned_key)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::new_map;

    #[test]
    fn test_msgpack_to_inner_simple() {
        let interner = Interner::new();

        // Create test data via rmpv
        let map = rmpv::Value::Map(vec![
            (rmpv::Value::from("name"), rmpv::Value::from("Alice")),
            (rmpv::Value::from("age"), rmpv::Value::from(30i64)),
        ]);

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &map).unwrap();

        let inner = msgpack_to_inner(&interner, &buf).unwrap();

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
    fn test_inner_to_msgpack_simple() {
        let interner = Interner::new();

        let name_key = interner.touch_ind("name").unwrap().key().clone();
        let age_key = interner.touch_ind("age").unwrap().key().clone();

        let mut map = new_map();
        map.insert(name_key, InnerValue::Str("Alice".to_string()));
        map.insert(age_key, InnerValue::Int(30));

        let inner = InnerValue::Map(map);
        let msgpack = inner_to_msgpack(&interner, &inner).unwrap();

        // Verify it's valid MessagePack
        let decoded: RmpvValue = rmpv::decode::read_value(&mut &*msgpack).unwrap();

        match decoded {
            RmpvValue::Map(m) => {
                let mut found_name = false;
                let mut found_age = false;

                for (key, val) in &m {
                    if let RmpvValue::String(k) = key {
                        if let Some("name") = k.as_str() {
                            found_name = true;
                            assert_eq!(val, &RmpvValue::String("Alice".into()));
                        } else if let Some("age") = k.as_str() {
                            found_age = true;
                            assert_eq!(val, &RmpvValue::Integer(30.into()));
                        }
                    }
                }

                assert!(found_name && found_age, "Expected both name and age keys");
            }
            _ => panic!("Expected Map"),
        }
    }

    #[test]
    fn test_roundtrip_msgpack() {
        let interner = Interner::new();

        // Create test data via rmpv
        let test_data = rmpv::Value::Map(vec![
            (rmpv::Value::from("name"), rmpv::Value::from("Alice")),
            (rmpv::Value::from("age"), rmpv::Value::from(30i64)),
            (rmpv::Value::from("active"), rmpv::Value::Boolean(true)),
            (rmpv::Value::from("tags"), rmpv::Value::Array(vec![
                rmpv::Value::from("rust"),
                rmpv::Value::from("db"),
            ])),
        ]);

        let mut original_buf = Vec::new();
        rmpv::encode::write_value(&mut original_buf, &test_data).unwrap();

        // Roundtrip via interned codec
        let inner = msgpack_to_inner(&interner, &original_buf).unwrap();
        let result_buf = inner_to_msgpack(&interner, &inner).unwrap();

        // Compare MessagePack bytes directly
        assert_eq!(original_buf, result_buf);
    }

    #[test]
    fn test_nested_structures() {
        let interner = Interner::new();

        let test_data = rmpv::Value::Map(vec![
            (rmpv::Value::from("user"), rmpv::Value::Map(vec![
                (rmpv::Value::from("name"), rmpv::Value::from("Bob")),
                (rmpv::Value::from("prefs"), rmpv::Value::Map(vec![
                    (rmpv::Value::from("theme"), rmpv::Value::from("dark")),
                ])),
            ])),
            (rmpv::Value::from("items"), rmpv::Value::Array(vec![
                rmpv::Value::from(1i64),
                rmpv::Value::from(2i64),
                rmpv::Value::from(3i64),
            ])),
        ]);

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &test_data).unwrap();

        let inner = msgpack_to_inner(&interner, &buf).unwrap();
        let result = inner_to_msgpack(&interner, &inner).unwrap();

        assert_eq!(buf, result);
    }

    #[test]
    fn test_interning_reuse() {
        let interner = Interner::new();

        let test_data = rmpv::Value::Map(vec![
            (rmpv::Value::from("key"), rmpv::Value::from("value")),
            (rmpv::Value::from("nested"), rmpv::Value::Map(vec![
                (rmpv::Value::from("key"), rmpv::Value::from("other")),
            ])),
        ]);

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &test_data).unwrap();

        msgpack_to_inner(&interner, &buf).unwrap();

        // "key" should be interned only once
        let key_interned = interner.touch_ind("key").unwrap();
        assert!(!key_interned.is_new(), "Key should already be interned");
    }

    #[test]
    fn test_binary_data() {
        let interner = Interner::new();

        let test_data = rmpv::Value::Map(vec![
            (rmpv::Value::from("data"), rmpv::Value::Binary(vec![1, 2, 3, 4, 5])),
        ]);

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &test_data).unwrap();

        let inner = msgpack_to_inner(&interner, &buf).unwrap();
        let result = inner_to_msgpack(&interner, &inner).unwrap();

        assert_eq!(buf, result);
    }

    #[test]
    fn test_all_msgpack_types() {
        let interner = Interner::new();

        let test_data = rmpv::Value::Map(vec![
            (rmpv::Value::from("null_val"), RmpvValue::Nil),
            (rmpv::Value::from("bool_true"), RmpvValue::Boolean(true)),
            (rmpv::Value::from("bool_false"), RmpvValue::Boolean(false)),
            (rmpv::Value::from("int_val"), RmpvValue::from(-42i64)),
            (rmpv::Value::from("uint_val"), RmpvValue::from(42u32)),
            (rmpv::Value::from("float_val"), RmpvValue::from(3.14f64)),
            (rmpv::Value::from("string_val"), RmpvValue::from("hello")),
            (rmpv::Value::from("binary_val"), RmpvValue::Binary(vec![1, 2, 3])),
            (rmpv::Value::from("array_val"), RmpvValue::Array(vec![
                RmpvValue::from(1i64),
                RmpvValue::from(2i64),
            ])),
        ]);

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &test_data).unwrap();

        let inner = msgpack_to_inner(&interner, &buf).unwrap();

        match inner {
            InnerValue::Map(m) => {
                assert_eq!(m.len(), 9);
            }
            _ => panic!("Expected Map"),
        }
    }

    #[test]
    fn test_empty_value() {
        let interner = Interner::new();

        let map = rmpv::Value::Map(vec![]);

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &map).unwrap();

        let inner = msgpack_to_inner(&interner, &buf).unwrap();
        let result = inner_to_msgpack(&interner, &inner).unwrap();

        assert_eq!(buf, result);
    }

    #[test]
    fn test_nil_value() {
        let interner = Interner::new();

        let inner = InnerValue::Nil;
        let msgpack = inner_to_msgpack(&interner, &inner).unwrap();

        let decoded: RmpvValue = rmpv::decode::read_value(&mut &*msgpack).unwrap();
        assert_eq!(decoded, RmpvValue::Nil);
    }

    #[test]
    fn test_large_unsigned_int() {
        let interner = Interner::new();

        // Large unsigned integer that doesn't fit in i64
        let large_u64: u64 = i64::MAX as u64 + 1;
        let test_data = rmpv::Value::Map(vec![
            (rmpv::Value::from("large"), RmpvValue::from(large_u64)),
        ]);

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &test_data).unwrap();

        let inner = msgpack_to_inner(&interner, &buf).unwrap();

        match inner {
            InnerValue::Map(m) => {
                let key = interner.touch_ind("large").unwrap().key().clone();
                match m.get(&key) {
                    Some(InnerValue::Str(s)) => {
                        assert_eq!(s, &large_u64.to_string());
                    }
                    other => panic!("Expected String for large unsigned int, got {:?}", other),
                }
            }
            _ => panic!("Expected Map"),
        }
    }

    #[test]
    fn test_error_invalid_msgpack() {
        let interner = Interner::new();
        // Array with 2 elements but only 1 provided (incomplete data)
        let invalid_msgpack = &[0x92, 0x01]; // [1, <missing>]

        let result = msgpack_to_inner(&interner, invalid_msgpack);
        assert!(result.is_err());
    }
}
