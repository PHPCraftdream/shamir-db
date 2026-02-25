use crate::codecs::interned::{inner_to_json, json_to_inner};
use crate::core::interner::Interner;
use crate::types::common::new_map;
use crate::types::value::InnerValue;

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
            assert_eq!(
                m.get(&name_key),
                Some(&InnerValue::Str("Alice".to_string()))
            );

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

#[test]
fn test_null_roundtrip() {
    let interner = Interner::new();

    let inner = InnerValue::Null;
    let json = inner_to_json(&interner, &inner).unwrap();
    let json_str = String::from_utf8(json).unwrap();
    assert_eq!(json_str, "null");

    let decoded = json_to_inner(&interner, json_str.as_bytes()).unwrap();
    assert_eq!(decoded, InnerValue::Null);
}

#[test]
fn test_null_in_map_roundtrip() {
    let interner = Interner::new();

    let json = r#"{"field": null}"#;

    let inner = json_to_inner(&interner, json.as_bytes()).unwrap();
    let result_json = inner_to_json(&interner, &inner).unwrap();

    let original_val: serde_json::Value = serde_json::from_str(json).unwrap();
    let result_val: serde_json::Value = serde_json::from_slice(&result_json).unwrap();
    assert_eq!(original_val, result_val);

    match inner {
        InnerValue::Map(m) => {
            let key = interner.touch_ind("field").unwrap().key().clone();
            assert_eq!(m.get(&key), Some(&InnerValue::Null));
        }
        _ => panic!("Expected Map"),
    }
}
