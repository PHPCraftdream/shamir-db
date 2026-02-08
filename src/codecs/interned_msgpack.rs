//! MessagePack codec with on-the-fly key interning.
//!
//! Functions to convert between MessagePack bytes and InnerValue (Value<u16>)
//! with string key interning.

use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::common::{new_map_wc, new_set_wc};
use crate::types::value::{InnerValue, Value};
use rust_decimal::Decimal;
use num_bigint::BigInt;
use std::str::FromStr;

/// Decode MessagePack bytes to InnerValue, interning string keys.
pub fn msgpack_to_inner(interner: &Interner, bytes: &[u8]) -> Result<InnerValue, CodecError> {
    // Parse MessagePack to rmpv::Value
    let mp_value: rmpv::Value = rmpv::decode::read_value(&mut bytes.as_ref())
        .map_err(|e| CodecError::Decode(e.to_string()))?;

    // Transform to InnerValue with interning
    Ok(transform_msgpack_to_inner(mp_value, interner))
}

/// Encode InnerValue to MessagePack bytes.
pub fn inner_to_msgpack(interner: &Interner, value: &InnerValue) -> Result<Vec<u8>, CodecError> {
    // Convert InnerValue to rmpv::Value with string keys
    let mp_value = inner_to_msgpack_value(value, interner);

    // Serialize to bytes
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &mp_value)
        .map_err(|e| CodecError::Encode(e.to_string()))?;
    Ok(buf)
}

/// Parse type prefix from key (e.g., "i:age" -> Some("i"), "age")
/// Used for explicit type hints in MessagePack field names.
fn parse_key_prefix(key: &str) -> (Option<&str>, &str) {
    if let Some((prefix, rest)) = key.split_once(':') {
        (Some(prefix), rest)
    } else {
        (None, key)
    }
}

/// Helper to convert rmpv::Utf8String to a regular String
fn utf8string_to_string(s: rmpv::Utf8String) -> String {
    s.as_str().unwrap_or("").to_string()
}

/// Helper to get &str from rmpv::Utf8String
fn utf8string_as_str(s: &rmpv::Utf8String) -> &str {
    s.as_str().unwrap_or("")
}

/// Transforms rmpv::Value to InnerValue, interning all string keys.
/// Supports type hints from field name prefixes (e.g., "i:age", "dec:price").
fn transform_msgpack_to_inner(value: rmpv::Value, interner: &Interner) -> InnerValue {
    match value {
        rmpv::Value::Nil => Value::Nil,
        rmpv::Value::Boolean(b) => Value::Bool(b),
        rmpv::Value::Integer(n) => {
            if n.is_i64() {
                Value::Int(n.as_i64().unwrap())
            } else {
                // Too large for i64, store as string
                Value::Str(n.to_string())
            }
        }
        rmpv::Value::F32(f) => Value::F64(f as f64),
        rmpv::Value::F64(f) => Value::F64(f),
        rmpv::Value::String(s) => Value::Str(utf8string_to_string(s)),
        rmpv::Value::Binary(b) => Value::Bin(b.into()),
        rmpv::Value::Array(arr) => {
            let inner_list = arr
                .into_iter()
                .map(|v| transform_msgpack_to_inner(v, interner))
                .collect();
            Value::List(inner_list)
        }
        rmpv::Value::Map(map) => {
            let mut inner_map = new_map_wc(map.len());
            for (key_val, val) in map {
                // Extract key as string if possible
                let key_str: Option<String> = match key_val {
                    rmpv::Value::String(s) => Some(utf8string_to_string(s)),
                    rmpv::Value::Integer(n) => Some(n.to_string()),
                    _ => None, // Non-string keys are skipped
                };

                if let Some(key) = key_str {
                    let (prefix, real_key) = parse_key_prefix(&key);

                    // Parse value based on prefix hint
                    let inner_val = match prefix {
                        Some("i") => {
                            // Explicit integer
                            match val {
                                rmpv::Value::Integer(n) => {
                                    if n.is_i64() {
                                        Value::Int(n.as_i64().unwrap())
                                    } else {
                                        Value::Str(n.to_string())
                                    }
                                }
                                rmpv::Value::F32(n) => Value::Int(n as i64),
                                rmpv::Value::F64(n) => Value::Int(n as i64),
                                rmpv::Value::String(s) => {
                                    let s_ref = utf8string_as_str(&s);
                                    s_ref.parse::<i64>().map(Value::Int).unwrap_or_else(|_| {
                                        s_ref.parse::<u64>().map(|v| Value::Int(v as i64))
                                            .unwrap_or(Value::Str(s_ref.to_string()))
                                    })
                                }
                                _ => transform_msgpack_to_inner(val, interner),
                            }
                        }
                        Some("u") => {
                            // Unsigned integer
                            match val {
                                rmpv::Value::Integer(n) => {
                                    if n.is_u64() {
                                        let u = n.as_u64().unwrap();
                                        if u <= i64::MAX as u64 {
                                            Value::Int(u as i64)
                                        } else {
                                            Value::Str(u.to_string())
                                        }
                                    } else if n.is_i64() {
                                        Value::Int(n.as_i64().unwrap())
                                    } else {
                                        Value::Str(n.to_string())
                                    }
                                }
                                rmpv::Value::String(s) => {
                                    let s_ref = utf8string_as_str(&s);
                                    s_ref.parse::<u64>().map(|v| {
                                        if v <= i64::MAX as u64 {
                                            Value::Int(v as i64)
                                        } else {
                                            Value::Str(v.to_string())
                                        }
                                    }).unwrap_or(Value::Str(s_ref.to_string()))
                                }
                                _ => transform_msgpack_to_inner(val, interner),
                            }
                        }
                        Some("float") => {
                            // Explicit float
                            match val {
                                rmpv::Value::F32(n) => Value::F64(n as f64),
                                rmpv::Value::F64(n) => Value::F64(n),
                                rmpv::Value::Integer(n) => {
                                    if n.is_i64() {
                                        Value::F64(n.as_i64().unwrap() as f64)
                                    } else {
                                        Value::F64(n.as_u64().unwrap() as f64)
                                    }
                                }
                                rmpv::Value::String(s) => {
                                    let s_ref = utf8string_as_str(&s);
                                    s_ref.parse::<f64>().map(Value::F64)
                                        .unwrap_or(Value::Str(s_ref.to_string()))
                                }
                                _ => transform_msgpack_to_inner(val, interner),
                            }
                        }
                        Some("dec") => {
                            // Decimal - store as validated string
                            match val {
                                rmpv::Value::String(s) => {
                                    let s_ref = utf8string_as_str(&s);
                                    // Validate decimal format
                                    let _ = Decimal::from_str(s_ref);
                                    Value::Str(s_ref.to_string())
                                }
                                rmpv::Value::Integer(n) => Value::Str(n.to_string()),
                                rmpv::Value::F32(n) => Value::Str(n.to_string()),
                                rmpv::Value::F64(n) => Value::Str(n.to_string()),
                                _ => transform_msgpack_to_inner(val, interner),
                            }
                        }
                        Some("big") => {
                            // BigInt - store as validated string
                            match val {
                                rmpv::Value::String(s) => {
                                    let s_ref = utf8string_as_str(&s);
                                    // Validate bigint format
                                    let _ = BigInt::from_str(s_ref);
                                    Value::Str(s_ref.to_string())
                                }
                                rmpv::Value::Integer(n) => Value::Str(n.to_string()),
                                rmpv::Value::F32(n) => Value::Str(n.to_string()),
                                rmpv::Value::F64(n) => Value::Str(n.to_string()),
                                _ => transform_msgpack_to_inner(val, interner),
                            }
                        }
                        Some("arr") => {
                            // Explicit array
                            match val {
                                rmpv::Value::Array(arr) => {
                                    let list = arr.into_iter()
                                        .map(|v| transform_msgpack_to_inner(v, interner))
                                        .collect();
                                    Value::List(list)
                                }
                                _ => transform_msgpack_to_inner(val, interner),
                            }
                        }
                        Some("set") => {
                            // Explicit set
                            match val {
                                rmpv::Value::Array(arr) => {
                                    let mut set = new_set_wc(arr.len());
                                    for v in arr {
                                        set.insert(transform_msgpack_to_inner(v, interner));
                                    }
                                    Value::Set(set)
                                }
                                _ => transform_msgpack_to_inner(val, interner),
                            }
                        }
                        Some(unknown) => {
                            // Unknown prefix - log warning but continue processing
                            eprintln!("warning: unknown type prefix: '{}'", unknown);
                            transform_msgpack_to_inner(val, interner)
                        }
                        None => {
                            // No prefix - auto-detect type
                            transform_msgpack_to_inner(val, interner)
                        }
                    };

                    let interned_key = interner
                        .touch_ind(&real_key)
                        .expect("failed to intern key")
                        .key()
                        .clone();
                    inner_map.insert(interned_key, inner_val);
                }
            }
            Value::Map(inner_map)
        }
        rmpv::Value::Ext(ty, _data) => {
            // Extension types - for now just log and treat as Nil
            eprintln!("warning: unsupported MessagePack ext type: {}", ty);
            Value::Nil
        }
    }
}

/// Convert InnerValue to rmpv::Value, converting numeric keys back to strings.
fn inner_to_msgpack_value(value: &InnerValue, interner: &Interner) -> rmpv::Value {
    match value {
        Value::Nil => rmpv::Value::Nil,
        Value::Bool(b) => rmpv::Value::Boolean(*b),
        Value::Int(i) => rmpv::Value::Integer((*i).into()),
        Value::F64(f) => rmpv::Value::F64(*f),
        Value::Dec(d) => rmpv::Value::String(d.to_string().into()),
        Value::Big(b) => rmpv::Value::String(b.to_string().into()),
        Value::Str(s) => rmpv::Value::String(s.clone().into()),
        Value::Bin(b) => rmpv::Value::Binary(b.clone().into()),
        Value::List(list) => {
            rmpv::Value::Array(list.iter().map(|v| inner_to_msgpack_value(v, interner)).collect())
        }
        Value::Set(set) => {
            rmpv::Value::Array(set.iter().map(|v| inner_to_msgpack_value(v, interner)).collect())
        }
        Value::Map(map) => {
            let mut mp_map = Vec::with_capacity(map.len());
            for (key_id, val) in map {
                // Look up string key from interner
                let key_str = match interner.get_str(key_id) {
                    Some(s) => s.to_string(),
                    None => format!("<key:{}>", key_id),
                };
                mp_map.push((rmpv::Value::String(key_str.into()), inner_to_msgpack_value(val, interner)));
            }
            rmpv::Value::Map(mp_map)
        }
    }
}

/// Helper to create MessagePack bytes from a value
pub fn to_msgpack<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    rmp_serde::to_vec(value).map_err(|e| CodecError::Encode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: rmpv::Value → MessagePack bytes
    fn to_msgpack_bytes(value: &rmpv::Value) -> Vec<u8> {
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, value).unwrap();
        buf
    }

    #[test]
    fn test_decode_simple_map() {
        let interner = Interner::new();

        // Create rmpv::Value directly
        let mp_value = rmpv::Value::Map(vec![
            (rmpv::Value::String("name".into()), rmpv::Value::String("Alice".into())),
        ]);
        let mp = to_msgpack_bytes(&mp_value);

        // Debug: check what we're encoding
        eprintln!("Encoded mp bytes: {:?}", mp);

        let result = msgpack_to_inner(&interner, &mp).unwrap();

        // Debug: check result
        eprintln!("Result: {:?}", result);

        // Debug: check interner state
        eprintln!("Interned keys: {:?}", interner.len());

        // Expected InnerValue - get the ID from the result
        if let InnerValue::Map(map) = &result {
            eprintln!("Result map keys: {:?}", map.keys().collect::<Vec<_>>());
            let name_id = map.keys().next().unwrap().clone();
            let mut expected_map = new_map_wc(1);
            expected_map.insert(name_id, InnerValue::Str("Alice".to_string()));
            let expected = InnerValue::Map(expected_map);
            assert_eq!(result, expected);
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_decode_multiple_keys() {
        let interner = Interner::new();

        let mp_value = rmpv::Value::Map(vec![
            (rmpv::Value::String("name".into()), rmpv::Value::String("Bob".into())),
            (rmpv::Value::String("age".into()), rmpv::Value::Integer(30.into())),
        ]);
        let mp = to_msgpack_bytes(&mp_value);
        let result = msgpack_to_inner(&interner, &mp).unwrap();

        let name_id = interner.get_ind("name").unwrap();
        let age_id = interner.get_ind("age").unwrap();
        let mut expected_map = new_map_wc(2);
        expected_map.insert(name_id, InnerValue::Str("Bob".to_string()));
        expected_map.insert(age_id, InnerValue::Int(30));
        let expected = InnerValue::Map(expected_map);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_all_value_types() {
        let interner = Interner::new();

        let mp_value = rmpv::Value::Array(vec![
            rmpv::Value::Nil,
            rmpv::Value::Boolean(true),
            rmpv::Value::Boolean(false),
            rmpv::Value::Integer(42.into()),
            rmpv::Value::F64(3.14),
            rmpv::Value::String("hello".into()),
            rmpv::Value::Array(vec![
                rmpv::Value::Integer(1.into()),
                rmpv::Value::Integer(2.into()),
            ]),
        ]);
        let mp = to_msgpack_bytes(&mp_value);
        let result = msgpack_to_inner(&interner, &mp).unwrap();

        let expected = InnerValue::List(vec![
            InnerValue::Nil,
            InnerValue::Bool(true),
            InnerValue::Bool(false),
            InnerValue::Int(42),
            InnerValue::F64(3.14),
            InnerValue::Str("hello".to_string()),
            InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
        ]);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_nested_map() {
        let interner = Interner::new();

        let inner_map = rmpv::Value::Map(vec![
            (rmpv::Value::String("name".into()), rmpv::Value::String("Bob".into())),
            (rmpv::Value::String("age".into()), rmpv::Value::Integer(30.into())),
        ]);
        let mp_value = rmpv::Value::Map(vec![
            (rmpv::Value::String("user".into()), inner_map),
        ]);
        let mp = to_msgpack_bytes(&mp_value);
        let result = msgpack_to_inner(&interner, &mp).unwrap();

        let user_id = interner.get_ind("user").unwrap();
        let name_id = interner.get_ind("name").unwrap();
        let age_id = interner.get_ind("age").unwrap();

        let mut nested_map = new_map_wc(2);
        nested_map.insert(name_id, InnerValue::Str("Bob".to_string()));
        nested_map.insert(age_id, InnerValue::Int(30));

        let mut expected_map = new_map_wc(1);
        expected_map.insert(user_id, InnerValue::Map(nested_map));
        let expected = InnerValue::Map(expected_map);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_type_hints() {
        let interner = Interner::new();

        let mp_value = rmpv::Value::Map(vec![
            (rmpv::Value::String("i:age".into()), rmpv::Value::String("30".into())),
            (rmpv::Value::String("u:count".into()), rmpv::Value::String("100".into())),
            (rmpv::Value::String("float:price".into()), rmpv::Value::String("19.99".into())),
            (rmpv::Value::String("dec:tax".into()), rmpv::Value::String("0.15".into())),
            (rmpv::Value::String("big:id".into()), rmpv::Value::String("12345678901234567890".into())),
        ]);
        let mp = to_msgpack_bytes(&mp_value);
        let result = msgpack_to_inner(&interner, &mp).unwrap();

        let age_id = interner.get_ind("age").unwrap();
        let count_id = interner.get_ind("count").unwrap();
        let price_id = interner.get_ind("price").unwrap();
        let tax_id = interner.get_ind("tax").unwrap();
        let id_id = interner.get_ind("id").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&age_id), Some(&InnerValue::Int(30)));
            assert_eq!(map.get(&count_id), Some(&InnerValue::Int(100)));
            assert_eq!(map.get(&price_id), Some(&InnerValue::F64(19.99)));
            assert_eq!(map.get(&tax_id), Some(&InnerValue::Str("0.15".to_string())));
            assert_eq!(map.get(&id_id), Some(&InnerValue::Str("12345678901234567890".to_string())));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_arr_and_set_hints() {
        let interner = Interner::new();

        let mp_value = rmpv::Value::Map(vec![
            (rmpv::Value::String("arr:items".into()), rmpv::Value::Array(vec![
                rmpv::Value::Integer(1.into()),
                rmpv::Value::Integer(2.into()),
                rmpv::Value::Integer(3.into()),
            ])),
            (rmpv::Value::String("set:tags".into()), rmpv::Value::Array(vec![
                rmpv::Value::String("a".into()),
                rmpv::Value::String("b".into()),
                rmpv::Value::String("c".into()),
            ])),
        ]);
        let mp = to_msgpack_bytes(&mp_value);
        let result = msgpack_to_inner(&interner, &mp).unwrap();

        let items_id = interner.get_ind("items").unwrap();
        let tags_id = interner.get_ind("tags").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&items_id), Some(&InnerValue::List(vec![
                InnerValue::Int(1),
                InnerValue::Int(2),
                InnerValue::Int(3),
            ])));
            assert!(matches!(map.get(&tags_id), Some(InnerValue::Set(_))));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_msgpack_with_unicode() {
        let interner = Interner::new();

        let mp_value = rmpv::Value::Map(vec![
            (rmpv::Value::String("привет".into()), rmpv::Value::String("мир".into())),
            (rmpv::Value::String("emoji".into()), rmpv::Value::String("🚀🎉".into())),
        ]);
        let mp = to_msgpack_bytes(&mp_value);
        let result = msgpack_to_inner(&interner, &mp).unwrap();

        let privet_id = interner.get_ind("привет").unwrap();
        let emoji_id = interner.get_ind("emoji").unwrap();
        let mut expected_map = new_map_wc(2);
        expected_map.insert(privet_id, InnerValue::Str("мир".to_string()));
        expected_map.insert(emoji_id, InnerValue::Str("🚀🎉".to_string()));
        let expected = InnerValue::Map(expected_map);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_msgpack_empty_map_and_array() {
        let interner = Interner::new();

        let empty_map = rmpv::Value::Map(vec![]);
        let mp1 = to_msgpack_bytes(&empty_map);
        let result1 = msgpack_to_inner(&interner, &mp1).unwrap();
        assert_eq!(result1, InnerValue::Map(new_map_wc(0)));

        let empty_array = rmpv::Value::Array(vec![]);
        let mp2 = to_msgpack_bytes(&empty_array);
        let result2 = msgpack_to_inner(&interner, &mp2).unwrap();
        assert_eq!(result2, InnerValue::List(vec![]));
    }

    #[test]
    fn test_inner_to_msgpack_roundtrip() {
        let interner = Interner::new();

        let mp_value = rmpv::Value::Map(vec![
            (rmpv::Value::String("name".into()), rmpv::Value::String("Alice".into())),
            (rmpv::Value::String("age".into()), rmpv::Value::Integer(30.into())),
        ]);
        let mp1 = to_msgpack_bytes(&mp_value);
        let inner = msgpack_to_inner(&interner, &mp1).unwrap();
        let mp2 = inner_to_msgpack(&interner, &inner).unwrap();

        // Both MessagePack should be equivalent
        assert_eq!(mp1, mp2);
    }

    #[test]
    fn test_binary_data() {
        let interner = Interner::new();

        let binary_data = vec![0u8, 1, 2, 3, 255, 254];
        let mp_value = rmpv::Value::Map(vec![
            (rmpv::Value::String("data".into()), rmpv::Value::Binary(binary_data.clone().into())),
        ]);
        let mp = to_msgpack_bytes(&mp_value);
        let result = msgpack_to_inner(&interner, &mp).unwrap();

        let data_id = interner.get_ind("data").unwrap();
        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&data_id), Some(&InnerValue::Bin(binary_data)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_large_uint() {
        let interner = Interner::new();

        // Test large u64 that doesn't fit in i64
        let large_uint = u64::MAX;
        let mp_value = rmpv::Value::Integer(large_uint.into());
        let mp = to_msgpack_bytes(&mp_value);
        let result = msgpack_to_inner(&interner, &mp).unwrap();

        // Should be stored as string since it doesn't fit in i64
        if let InnerValue::Str(s) = result {
            assert_eq!(s, large_uint.to_string());
        } else {
            panic!("Expected Str for large u64");
        }
    }
}
