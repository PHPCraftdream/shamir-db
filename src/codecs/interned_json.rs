//! JSON codec with on-the-fly key interning.
//!
//! Functions to convert between JSON bytes and InnerValue (Value<u16>)
//! with string key interning.

use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::common::{new_map_wc, new_set_wc};
use crate::types::value::{InnerValue, Value};
use rust_decimal::Decimal;
use num_bigint::BigInt;
use std::str::FromStr;

/// Decode JSON bytes to InnerValue, interning string keys.
pub fn json_to_inner(interner: &Interner, bytes: &[u8]) -> Result<InnerValue, CodecError> {
    // Parse JSON to serde_json::Value
    let json_value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| CodecError::Decode(e.to_string()))?;

    // Transform to InnerValue with interning
    Ok(transform_json_to_inner(json_value, interner))
}

/// Encode InnerValue to JSON bytes.
pub fn inner_to_json(interner: &Interner, value: &InnerValue) -> Result<Vec<u8>, CodecError> {
    // Convert InnerValue to serde_json::Value with string keys
    let json_value = inner_to_json_value(value, interner);
    serde_json::to_vec(&json_value).map_err(|e| CodecError::Encode(e.to_string()))
}

/// Parse type prefix from key (e.g., "i:age" -> Some("i"), "age")
/// Used for explicit type hints in JSON field names.
fn parse_key_prefix(key: &str) -> (Option<&str>, &str) {
    if let Some((prefix, rest)) = key.split_once(':') {
        (Some(prefix), rest)
    } else {
        (None, key)
    }
}

/// Transforms serde_json::Value to InnerValue, interning all string keys.
/// Supports type hints from field name prefixes (e.g., "i:age", "dec:price").
fn transform_json_to_inner(value: serde_json::Value, interner: &Interner) -> InnerValue {
    match value {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                Value::Int(u as i64)
            } else {
                Value::F64(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::Str(s),
        serde_json::Value::Array(arr) => {
            let inner_list = arr
                .into_iter()
                .map(|v| transform_json_to_inner(v, interner))
                .collect();
            Value::List(inner_list)
        }
        serde_json::Value::Object(obj) => {
            let mut inner_map = new_map_wc(obj.len());
            for (key, val) in obj {
                let (prefix, real_key) = parse_key_prefix(&key);

                // Parse value based on prefix hint
                let inner_val = match prefix {
                    Some("i") => {
                        // Explicit integer
                        match val {
                            serde_json::Value::Number(n) => {
                                if let Some(i) = n.as_i64() {
                                    Value::Int(i)
                                } else if let Some(u) = n.as_u64() {
                                    Value::Int(u as i64)
                                } else {
                                    Value::F64(n.as_f64().unwrap_or(0.0))
                                }
                            }
                            serde_json::Value::String(s) => {
                                // Try to parse string as integer
                                s.parse::<i64>().map(Value::Int).unwrap_or_else(|_| {
                                    s.parse::<u64>().map(|v| Value::Int(v as i64))
                                        .unwrap_or(Value::Str(s))
                                })
                            }
                            _ => transform_json_to_inner(val, interner),
                        }
                    }
                    Some("u") => {
                        // Unsigned integer
                        match val {
                            serde_json::Value::Number(n) => {
                                n.as_u64().map(|v| Value::Int(v as i64))
                                    .unwrap_or_else(|| Value::F64(n.as_f64().unwrap_or(0.0)))
                            }
                            serde_json::Value::String(s) => {
                                s.parse::<u64>().map(|v| Value::Int(v as i64))
                                    .unwrap_or(Value::Str(s))
                            }
                            _ => transform_json_to_inner(val, interner),
                        }
                    }
                    Some("float") => {
                        // Explicit float
                        match val {
                            serde_json::Value::Number(n) => Value::F64(n.as_f64().unwrap_or(0.0)),
                            serde_json::Value::String(s) => {
                                s.parse::<f64>().map(Value::F64).unwrap_or(Value::Str(s))
                            }
                            _ => transform_json_to_inner(val, interner),
                        }
                    }
                    Some("dec") => {
                        // Decimal - store as validated string
                        match val {
                            serde_json::Value::String(s) => {
                                // Validate decimal format
                                let _ = Decimal::from_str(&s);
                                Value::Str(s)
                            }
                            serde_json::Value::Number(n) => {
                                Value::Str(n.to_string())
                            }
                            _ => transform_json_to_inner(val, interner),
                        }
                    }
                    Some("big") => {
                        // BigInt - store as validated string
                        match val {
                            serde_json::Value::String(s) => {
                                // Validate bigint format
                                let _ = BigInt::from_str(&s);
                                Value::Str(s)
                            }
                            serde_json::Value::Number(n) => {
                                if let Some(i) = n.as_i64() {
                                    Value::Str(i.to_string())
                                } else if let Some(u) = n.as_u64() {
                                    Value::Str(u.to_string())
                                } else {
                                    Value::Str(n.as_f64().unwrap_or(0.0).to_string())
                                }
                            }
                            _ => transform_json_to_inner(val, interner),
                        }
                    }
                    Some("arr") => {
                        // Explicit array
                        match val {
                            serde_json::Value::Array(arr) => {
                                let list = arr.into_iter()
                                    .map(|v| transform_json_to_inner(v, interner))
                                    .collect();
                                Value::List(list)
                            }
                            _ => transform_json_to_inner(val, interner),
                        }
                    }
                    Some("set") => {
                        // Explicit set
                        match val {
                            serde_json::Value::Array(arr) => {
                                let mut set = new_set_wc(arr.len());
                                for v in arr {
                                    set.insert(transform_json_to_inner(v, interner));
                                }
                                Value::Set(set)
                            }
                            _ => transform_json_to_inner(val, interner),
                        }
                    }
                    Some(unknown) => {
                        // Unknown prefix - log warning but continue processing
                        eprintln!("warning: unknown type prefix: '{}'", unknown);
                        transform_json_to_inner(val, interner)
                    }
                    None => {
                        // No prefix - auto-detect type
                        transform_json_to_inner(val, interner)
                    }
                };

                let interned_key = interner
                    .touch_ind(&real_key)
                    .expect("failed to intern key")
                    .val();
                inner_map.insert(interned_key, inner_val);
            }
            Value::Map(inner_map)
        }
    }
}

/// Convert InnerValue to serde_json::Value, converting numeric keys back to strings.
fn inner_to_json_value(value: &InnerValue, interner: &Interner) -> serde_json::Value {
    match value {
        Value::Nil => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number(serde_json::Number::from(*i)),
        Value::F64(f) => serde_json::Value::Number(serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0))),
        Value::Dec(d) => serde_json::Value::String(d.to_string()),
        Value::Big(b) => serde_json::Value::String(b.to_string()),
        Value::Str(s) => serde_json::Value::String(s.clone()),
        Value::Bin(b) => serde_json::Value::Array(b.iter().map(|v| serde_json::Value::Number(serde_json::Number::from(*v))).collect()),
        Value::List(list) => {
            serde_json::Value::Array(list.iter().map(|v| inner_to_json_value(v, interner)).collect())
        }
        Value::Set(set) => {
            serde_json::Value::Array(set.iter().map(|v| inner_to_json_value(v, interner)).collect())
        }
        Value::Map(map) => {
            let mut obj = serde_json::map::Map::with_capacity(map.len());
            for (key_id, val) in map {
                // Look up string key from interner
                let key_str = match interner.get_str(*key_id) {
                    Some(s) => s.to_string(),
                    None => format!("<key:{}>", key_id),
                };
                obj.insert(key_str, inner_to_json_value(val, interner));
            }
            serde_json::Value::Object(obj)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::new_map;
    use crate::types::value::UserValue;

    /// Helper: UserValue → JSON bytes
    fn to_json(value: &UserValue) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    #[test]
    fn test_decode_simple_map() {
        let interner = Interner::new();

        // Create UserValue (as JSON constructor)
        let mut user_map = new_map();
        user_map.insert("name".to_string(), UserValue::Str("Alice".to_string()));
        let user_value = UserValue::Map(user_map);

        // UserValue → JSON → InnerValue
        let json = to_json(&user_value);
        let result = json_to_inner(&interner, &json).unwrap();

        // Expected InnerValue
        let name_id = interner.get_ind("name").expect("name should be interned");
        let mut expected_map = new_map_wc(1);
        expected_map.insert(name_id, InnerValue::Str("Alice".to_string()));
        let expected = InnerValue::Map(expected_map);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_decode_multiple_keys() {
        let interner = Interner::new();

        // Create UserValue with multiple keys
        let mut user_map = new_map();
        user_map.insert("name".to_string(), UserValue::Str("Bob".to_string()));
        user_map.insert("age".to_string(), UserValue::Int(30));
        let user_value = UserValue::Map(user_map);

        // UserValue → JSON → InnerValue
        let json = to_json(&user_value);
        let result = json_to_inner(&interner, &json).unwrap();

        // Expected InnerValue
        let name_id = interner.get_ind("name").unwrap();
        let age_id = interner.get_ind("age").unwrap();
        let mut expected_map = new_map_wc(2);
        expected_map.insert(name_id, InnerValue::Str("Bob".to_string()));
        expected_map.insert(age_id, InnerValue::Int(30));
        let expected = InnerValue::Map(expected_map);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_type_hints() {
        let interner = Interner::new();

        // JSON with type hints in field names
        let json = br#"{"i:age":"30","u:count":"100","float:price":"19.99","dec:tax":"0.15","big:id":"12345678901234567890"}"#;

        let result = json_to_inner(&interner, json).unwrap();

        // Check all keys were interned (without prefixes)
        let age_id = interner.get_ind("age").unwrap();
        let count_id = interner.get_ind("count").unwrap();
        let price_id = interner.get_ind("price").unwrap();
        let tax_id = interner.get_ind("tax").unwrap();
        let id_id = interner.get_ind("id").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&age_id), Some(&InnerValue::Int(30)));
            assert_eq!(map.get(&count_id), Some(&InnerValue::Int(100)));
            assert_eq!(map.get(&price_id), Some(&InnerValue::F64(19.99)));
            // dec and big are stored as validated strings
            assert_eq!(map.get(&tax_id), Some(&InnerValue::Str("0.15".to_string())));
            assert_eq!(map.get(&id_id), Some(&InnerValue::Str("12345678901234567890".to_string())));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_arr_and_set_hints() {
        let interner = Interner::new();

        // JSON with arr and set hints
        let json = br#"{"arr:items":[1,2,3],"set:tags":["a","b","c"]}"#;

        let result = json_to_inner(&interner, json).unwrap();

        let items_id = interner.get_ind("items").unwrap();
        let tags_id = interner.get_ind("tags").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&items_id), Some(&InnerValue::List(vec![
                InnerValue::Int(1),
                InnerValue::Int(2),
                InnerValue::Int(3),
            ])));
            // Set should be stored as Set
            assert!(matches!(map.get(&tags_id), Some(InnerValue::Set(_))));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_nested_map() {
        let interner = Interner::new();

        // Create nested UserValue: {"user": {"name": "Bob", "age": 30}}
        let mut inner_map = new_map();
        inner_map.insert("name".to_string(), UserValue::Str("Bob".to_string()));
        inner_map.insert("age".to_string(), UserValue::Int(30));

        let mut user_map = new_map();
        user_map.insert("user".to_string(), UserValue::Map(inner_map));
        let user_value = UserValue::Map(user_map);

        // UserValue → JSON → InnerValue
        let json = to_json(&user_value);
        let result = json_to_inner(&interner, &json).unwrap();

        // Expected InnerValue
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
    fn test_all_value_types() {
        let interner = Interner::new();

        // Create UserValue with various types
        let user_value = UserValue::List(vec![
            UserValue::Nil,
            UserValue::Bool(true),
            UserValue::Bool(false),
            UserValue::Int(42),
            UserValue::F64(3.14),
            UserValue::Str("hello".to_string()),
            UserValue::List(vec![UserValue::Int(1), UserValue::Int(2)]),
        ]);

        // UserValue → JSON → InnerValue
        let json = to_json(&user_value);
        let result = json_to_inner(&interner, &json).unwrap();

        // Expected InnerValue
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
    fn test_interning_is_deterministic() {
        let interner = Interner::new();

        // First call
        let mut map1 = new_map();
        map1.insert("name".to_string(), UserValue::Str("Alice".to_string()));
        let json1 = to_json(&UserValue::Map(map1));
        json_to_inner(&interner, &json1).unwrap();

        // Second call with same key
        let mut map2 = new_map();
        map2.insert("name".to_string(), UserValue::Str("Bob".to_string()));
        let json2 = to_json(&UserValue::Map(map2));
        json_to_inner(&interner, &json2).unwrap();

        // "name" should have the same ID (first key gets ID 1)
        let name_id = interner.get_ind("name").unwrap();
        assert_eq!(name_id, 1);
    }

    #[test]
    fn test_multiple_calls_with_same_keys() {
        let interner = Interner::new();

        // First call
        let mut map1 = new_map();
        map1.insert("name".to_string(), UserValue::Str("Alice".to_string()));
        map1.insert("email".to_string(), UserValue::Str("alice@example.com".to_string()));
        let json1 = to_json(&UserValue::Map(map1));
        json_to_inner(&interner, &json1).unwrap();

        // Second call with overlapping keys
        let mut map2 = new_map();
        map2.insert("name".to_string(), UserValue::Str("Bob".to_string()));
        map2.insert("age".to_string(), UserValue::Int(25));
        let json2 = to_json(&UserValue::Map(map2));
        let result = json_to_inner(&interner, &json2).unwrap();

        // Check that keys are shared (JSON objects iterate in alphabetical order)
        assert_eq!(interner.get_ind("email"), Some(1));
        assert_eq!(interner.get_ind("name"), Some(2));
        assert_eq!(interner.get_ind("age"), Some(3));

        // Expected result
        let name_id = interner.get_ind("name").unwrap();
        let age_id = interner.get_ind("age").unwrap();
        let mut expected_map = new_map_wc(2);
        expected_map.insert(name_id, InnerValue::Str("Bob".to_string()));
        expected_map.insert(age_id, InnerValue::Int(25));
        let expected = InnerValue::Map(expected_map);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_json_with_unicode() {
        let interner = Interner::new();

        // Create UserValue with unicode
        let mut user_map = new_map();
        user_map.insert("привет".to_string(), UserValue::Str("мир".to_string()));
        user_map.insert("emoji".to_string(), UserValue::Str("🚀🎉".to_string()));
        let user_value = UserValue::Map(user_map);

        // UserValue → JSON → InnerValue
        let json = to_json(&user_value);
        let result = json_to_inner(&interner, &json).unwrap();

        // Expected InnerValue
        let privet_id = interner.get_ind("привет").unwrap();
        let emoji_id = interner.get_ind("emoji").unwrap();
        let mut expected_map = new_map_wc(2);
        expected_map.insert(privet_id, InnerValue::Str("мир".to_string()));
        expected_map.insert(emoji_id, InnerValue::Str("🚀🎉".to_string()));
        let expected = InnerValue::Map(expected_map);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_json_empty_map_and_array() {
        let interner = Interner::new();

        // Empty map
        let user_value1 = UserValue::Map(new_map());
        let json1 = to_json(&user_value1);
        let result1 = json_to_inner(&interner, &json1).unwrap();
        assert_eq!(result1, InnerValue::Map(new_map_wc(0)));

        // Empty list
        let user_value2 = UserValue::List(vec![]);
        let json2 = to_json(&user_value2);
        let result2 = json_to_inner(&interner, &json2).unwrap();
        assert_eq!(result2, InnerValue::List(vec![]));
    }

    // ===== Type hint prefix tests =====

    #[test]
    fn test_prefix_i_integer_from_string() {
        let interner = Interner::new();
        // i: prefix should parse string as integer
        let json = br#"{"i:age":"30","i:count":"-5","i:zero":"0"}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let age_id = interner.get_ind("age").unwrap();
        let count_id = interner.get_ind("count").unwrap();
        let zero_id = interner.get_ind("zero").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&age_id), Some(&InnerValue::Int(30)));
            assert_eq!(map.get(&count_id), Some(&InnerValue::Int(-5)));
            assert_eq!(map.get(&zero_id), Some(&InnerValue::Int(0)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_i_integer_from_number() {
        let interner = Interner::new();
        // i: prefix with actual JSON number
        let json = br#"{"i:age":30,"i:large":9007199254740991}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let age_id = interner.get_ind("age").unwrap();
        let large_id = interner.get_ind("large").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&age_id), Some(&InnerValue::Int(30)));
            assert_eq!(map.get(&large_id), Some(&InnerValue::Int(9007199254740991)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_u_unsigned() {
        let interner = Interner::new();
        // u: prefix for unsigned integers
        let json = br#"{"u:count":"100","u:max":"18446744073709551615"}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let count_id = interner.get_ind("count").unwrap();
        let max_id = interner.get_ind("max").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&count_id), Some(&InnerValue::Int(100)));
            // Max u64 fits in i64 as negative, but we store as i64
            assert_eq!(map.get(&max_id), Some(&InnerValue::Int(-1)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_u_unsigned_from_number() {
        let interner = Interner::new();
        let json = br#"{"u:value":42}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let value_id = interner.get_ind("value").unwrap();
        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&value_id), Some(&InnerValue::Int(42)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_float() {
        let interner = Interner::new();
        // float: prefix for floating point
        let json = br#"{"float:price":"19.99","float:pi":"3.14159","float:negative":"-2.5"}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let price_id = interner.get_ind("price").unwrap();
        let pi_id = interner.get_ind("pi").unwrap();
        let negative_id = interner.get_ind("negative").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&price_id), Some(&InnerValue::F64(19.99)));
            assert_eq!(map.get(&pi_id), Some(&InnerValue::F64(3.14159)));
            assert_eq!(map.get(&negative_id), Some(&InnerValue::F64(-2.5)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_float_from_number() {
        let interner = Interner::new();
        let json = br#"{"float:value":3.14}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let value_id = interner.get_ind("value").unwrap();
        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&value_id), Some(&InnerValue::F64(3.14)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_dec_decimal() {
        let interner = Interner::new();
        // dec: prefix stores as validated string
        let json = br#"{"dec:price":"19.99","dec:tax":"0.0875","dec:negative":"-10.50"}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let price_id = interner.get_ind("price").unwrap();
        let tax_id = interner.get_ind("tax").unwrap();
        let negative_id = interner.get_ind("negative").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&price_id), Some(&InnerValue::Str("19.99".to_string())));
            assert_eq!(map.get(&tax_id), Some(&InnerValue::Str("0.0875".to_string())));
            assert_eq!(map.get(&negative_id), Some(&InnerValue::Str("-10.50".to_string())));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_dec_from_number() {
        let interner = Interner::new();
        let json = br#"{"dec:value":123}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let value_id = interner.get_ind("value").unwrap();
        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&value_id), Some(&InnerValue::Str("123".to_string())));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_big_bigint() {
        let interner = Interner::new();
        // big: prefix for very large integers, stored as string
        let json = br#"{"big:id":"123456789012345678901234567890","big:negative":"-98765432109876543210"}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let id_id = interner.get_ind("id").unwrap();
        let negative_id = interner.get_ind("negative").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&id_id), Some(&InnerValue::Str("123456789012345678901234567890".to_string())));
            assert_eq!(map.get(&negative_id), Some(&InnerValue::Str("-98765432109876543210".to_string())));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_big_from_number() {
        let interner = Interner::new();
        let json = br#"{"big:value":42}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let value_id = interner.get_ind("value").unwrap();
        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&value_id), Some(&InnerValue::Str("42".to_string())));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_arr_array() {
        let interner = Interner::new();
        // arr: prefix explicitly creates a List
        let json = br#"{"arr:numbers":[1,2,3],"arr:nested":[[1,2],[3,4]],"arr:empty":[]}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let numbers_id = interner.get_ind("numbers").unwrap();
        let nested_id = interner.get_ind("nested").unwrap();
        let empty_id = interner.get_ind("empty").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&numbers_id), Some(&InnerValue::List(vec![
                InnerValue::Int(1),
                InnerValue::Int(2),
                InnerValue::Int(3),
            ])));
            assert_eq!(map.get(&empty_id), Some(&InnerValue::List(vec![])));
            // Nested arrays
            if let Some(InnerValue::List(nested)) = map.get(&nested_id) {
                assert_eq!(nested.len(), 2);
            } else {
                panic!("Expected nested List");
            }
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_set() {
        let interner = Interner::new();
        // set: prefix creates a Set
        let json = br#"{"set:tags":["a","b","c"],"set:numbers":[1,2,3],"set:empty":[]}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let tags_id = interner.get_ind("tags").unwrap();
        let numbers_id = interner.get_ind("numbers").unwrap();
        let empty_id = interner.get_ind("empty").unwrap();

        if let InnerValue::Map(map) = result {
            // All should be Set type
            assert!(matches!(map.get(&tags_id), Some(InnerValue::Set(_))));
            assert!(matches!(map.get(&numbers_id), Some(InnerValue::Set(_))));
            assert!(matches!(map.get(&empty_id), Some(InnerValue::Set(_))));

            // Check Set content
            if let Some(InnerValue::Set(tags)) = map.get(&tags_id) {
                assert!(tags.contains(&InnerValue::Str("a".to_string())));
                assert!(tags.contains(&InnerValue::Str("b".to_string())));
                assert!(tags.contains(&InnerValue::Str("c".to_string())));
            }
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_mixed_with_regular_fields() {
        let interner = Interner::new();
        // Mix prefixed and non-prefixed fields
        let json = br#"{"name":"Alice","i:age":"30","city":"Minsk","float:score":"95.5"}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let name_id = interner.get_ind("name").unwrap();
        let age_id = interner.get_ind("age").unwrap();
        let city_id = interner.get_ind("city").unwrap();
        let score_id = interner.get_ind("score").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&name_id), Some(&InnerValue::Str("Alice".to_string())));
            assert_eq!(map.get(&age_id), Some(&InnerValue::Int(30)));
            assert_eq!(map.get(&city_id), Some(&InnerValue::Str("Minsk".to_string())));
            assert_eq!(map.get(&score_id), Some(&InnerValue::F64(95.5)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_nested_objects() {
        let interner = Interner::new();
        // Prefixes should work in nested objects
        let json = br#"{"user":{"i:age":"30","name":"Bob"},"set:tags":["a","b"]}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let user_id = interner.get_ind("user").unwrap();
        let tags_id = interner.get_ind("tags").unwrap();

        if let InnerValue::Map(map) = result {
            // Nested user map
            if let Some(InnerValue::Map(user_map)) = map.get(&user_id) {
                let age_id = interner.get_ind("age").unwrap();
                let name_id = interner.get_ind("name").unwrap();
                assert_eq!(user_map.get(&age_id), Some(&InnerValue::Int(30)));
                assert_eq!(user_map.get(&name_id), Some(&InnerValue::Str("Bob".to_string())));
            } else {
                panic!("Expected nested Map for user");
            }
            // Tags should be a Set
            assert!(matches!(map.get(&tags_id), Some(InnerValue::Set(_))));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_with_colon_in_value() {
        let interner = Interner::new();
        // Prefix only applies to key, not value
        let json = br#"{"i:value":"30","text":"value:with:colons"}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let value_id = interner.get_ind("value").unwrap();
        let text_id = interner.get_ind("text").unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.get(&value_id), Some(&InnerValue::Int(30)));
            assert_eq!(map.get(&text_id), Some(&InnerValue::Str("value:with:colons".to_string())));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_invalid_string_to_number() {
        let interner = Interner::new();
        // Invalid number strings fall back to String
        let json = br#"{"i:age":"not_a_number","u:count":"abc","float:price":"invalid"}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let age_id = interner.get_ind("age").unwrap();
        let count_id = interner.get_ind("count").unwrap();
        let price_id = interner.get_ind("price").unwrap();

        if let InnerValue::Map(map) = result {
            // All should fall back to String on parse failure
            assert_eq!(map.get(&age_id), Some(&InnerValue::Str("not_a_number".to_string())));
            assert_eq!(map.get(&count_id), Some(&InnerValue::Str("abc".to_string())));
            assert_eq!(map.get(&price_id), Some(&InnerValue::Str("invalid".to_string())));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_prefix_arr_set_with_non_array_values() {
        let interner = Interner::new();
        // arr/set with non-array value falls back to auto-detect
        let json = br#"{"arr:value":"not_array","set:value":"not_set"}"#;
        let result = json_to_inner(&interner, json).unwrap();

        let value_id = interner.get_ind("value").unwrap();

        if let InnerValue::Map(map) = result {
            // Should have two separate "value" entries due to prefix stripping
            // But since keys are the same, second overwrites first
            assert_eq!(map.get(&value_id), Some(&InnerValue::Str("not_set".to_string())));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_inner_to_json_roundtrip() {
        let interner = Interner::new();

        // Create UserValue
        let mut user_map = new_map();
        user_map.insert("name".to_string(), UserValue::Str("Alice".to_string()));
        user_map.insert("age".to_string(), UserValue::Int(30));
        let user_value = UserValue::Map(user_map);

        // UserValue → JSON → InnerValue → JSON
        let json1 = to_json(&user_value);
        let inner = json_to_inner(&interner, &json1).unwrap();
        let json2 = inner_to_json(&interner, &inner).unwrap();

        // Both JSON should be equivalent (may differ in whitespace/ordering)
        let v1: serde_json::Value = serde_json::from_slice(&json1).unwrap();
        let v2: serde_json::Value = serde_json::from_slice(&json2).unwrap();
        assert_eq!(v1, v2);
    }
}
