//! JSON codec with on-the-fly key interning.
//!
//! Functions to convert between JSON bytes and InnerValue (Value<u16>)
//! with string key interning.

use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::common::new_map_wc;
use crate::types::value::{InnerValue, Value, UserValue};

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

/// Transforms serde_json::Value to InnerValue, interning all string keys.
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
                let interned_key = interner
                    .touch_ind(&key)
                    .expect("failed to intern key")
                    .val();
                let inner_val = transform_json_to_inner(val, interner);
                inner_map.insert(interned_key, inner_val);
            }
            Value::Map(inner_map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::new_map;

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
