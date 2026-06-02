use crate::codecs::interned::json::{
    inner_to_json, inner_to_json_value, json_to_inner, json_value_to_inner_with,
};
use crate::core::interner::Interner;
use crate::core::interner::InternerKey;
use crate::types::common::{new_map, new_set};
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

#[test]
fn json_value_to_inner_with_custom_intern() {
    use crate::codecs::CodecError;
    use std::sync::atomic::{AtomicU64, Ordering};

    let counter = AtomicU64::new(1000);
    let intern = |_key: &str| -> Result<InternerKey, CodecError> {
        let id = counter.fetch_add(1, Ordering::SeqCst);
        Ok(InternerKey::new(id))
    };
    let json = serde_json::json!({
        "name": "alice",
        "age": 30
    });
    let inner = json_value_to_inner_with(&json, &intern).unwrap();
    assert!(matches!(inner, InnerValue::Map(_)));
}

// ---------------------------------------------------------------------------
// Scalar round-trips via inner_to_json / json_to_inner
// ---------------------------------------------------------------------------

#[test]
fn test_bool_roundtrip() {
    let interner = Interner::new();
    for b in [true, false] {
        let original = InnerValue::Bool(b);
        let encoded = inner_to_json(&interner, &original).unwrap();
        let decoded = json_to_inner(&interner, &encoded).unwrap();
        assert_eq!(decoded, original);
    }
}

#[test]
fn test_int_roundtrip_boundary_values() {
    let interner = Interner::new();
    for &v in &[
        0i64,
        1,
        -1,
        i64::MIN,
        i64::MAX,
        i32::MIN as i64,
        i32::MAX as i64,
    ] {
        let original = InnerValue::Int(v);
        let encoded = inner_to_json(&interner, &original).unwrap();
        let decoded = json_to_inner(&interner, &encoded).unwrap();
        assert_eq!(decoded, original, "int roundtrip failed for {}", v);
    }
}

#[test]
fn test_f64_roundtrip() {
    let interner = Interner::new();
    for &v in &[0.0f64, -0.0, 1.5, -99.999, f64::MIN_POSITIVE, 1e300] {
        let original = InnerValue::F64(v);
        let encoded = inner_to_json(&interner, &original).unwrap();
        let decoded = json_to_inner(&interner, &encoded).unwrap();
        assert_eq!(decoded, original, "f64 roundtrip for {}", v);
    }
}

#[test]
fn test_f64_non_finite_serialized_as_string() {
    let interner = Interner::new();
    // serde_json refuses non-finite floats, so our codec writes them as strings
    for &v in &[f64::INFINITY, f64::NEG_INFINITY] {
        let original = InnerValue::F64(v);
        let encoded = inner_to_json(&interner, &original).unwrap();
        // Should produce a JSON string like "inf" / "-inf"
        let json_val: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert!(json_val.is_string(), "non-finite should be a JSON string");
    }
}

#[test]
fn test_string_roundtrip_varieties() {
    let interner = Interner::new();
    let cases = vec![
        String::new(),
        "hello".to_string(),
        "Привет".to_string(),
        "🚀🎉🔥".to_string(),
        "a\"b\\c/d".to_string(),
        "\t\n\r".to_string(),
        "\x00\x01\x02".to_string(),
        "x".repeat(10_000),
    ];
    for s in cases {
        let original = InnerValue::Str(s.clone());
        let encoded = inner_to_json(&interner, &original).unwrap();
        let decoded = json_to_inner(&interner, &encoded).unwrap();
        assert_eq!(decoded, original, "string roundtrip for len={}", s.len());
    }
}

#[test]
fn test_bin_roundtrip() {
    let interner = Interner::new();
    let cases: Vec<Vec<u8>> = vec![
        vec![0],
        vec![255],
        vec![0, 1, 2, 3, 4, 5],
        (0..=255).collect(),
    ];
    for b in cases {
        let original = InnerValue::Bin(b.clone());
        let encoded = inner_to_json(&interner, &original).unwrap();
        let decoded = json_to_inner(&interner, &encoded).unwrap();
        // JSON has no binary type — Bin encodes as array of numbers.
        // After round-trip, arrays become List. We verify the values survive.
        match decoded {
            InnerValue::List(arr) => {
                assert_eq!(arr.len(), b.len());
                for (i, v) in arr.iter().enumerate() {
                    assert_eq!(*v, InnerValue::Int(b[i] as i64));
                }
            }
            InnerValue::Bin(decoded_b) => {
                assert_eq!(decoded_b, b);
            }
            other => panic!("Expected List or Bin, got {:?}", other),
        }
    }
}

#[test]
fn test_list_roundtrip() {
    let interner = Interner::new();
    let empty = InnerValue::List(vec![]);
    let encoded = inner_to_json(&interner, &empty).unwrap();
    let decoded = json_to_inner(&interner, &encoded).unwrap();
    assert_eq!(decoded, empty);

    let nested = InnerValue::List(vec![
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
        InnerValue::List(vec![]),
        InnerValue::Null,
    ]);
    let encoded = inner_to_json(&interner, &nested).unwrap();
    let decoded = json_to_inner(&interner, &encoded).unwrap();
    assert_eq!(decoded, nested);
}

#[test]
fn test_set_roundtrip() {
    let interner = Interner::new();
    let mut set = new_set();
    set.insert(InnerValue::Int(1));
    set.insert(InnerValue::Str("hello".to_string()));
    let original = InnerValue::Set(set);
    let encoded = inner_to_json(&interner, &original).unwrap();
    // Sets serialize as arrays; decode produces a List, not a Set
    let decoded = json_to_inner(&interner, &encoded).unwrap();
    match decoded {
        InnerValue::List(_) => {} // expected: sets become lists in JSON
        other => panic!("Expected List (from Set), got {:?}", other),
    }
}

#[test]
fn test_map_roundtrip_empty_and_nested() {
    let interner = Interner::new();
    // Empty map
    let empty = InnerValue::Map(new_map());
    let encoded = inner_to_json(&interner, &empty).unwrap();
    assert_eq!(encoded, b"{}");

    // Nested map
    let _k1 = interner.touch_ind("outer").unwrap().into_key();
    let k2 = interner.touch_ind("inner").unwrap().into_key();
    let k3 = interner.touch_ind("val").unwrap().into_key();
    let mut inner_map = new_map();
    inner_map.insert(k3, InnerValue::Int(42));
    let mut outer_map = new_map();
    outer_map.insert(k2, InnerValue::Map(inner_map));
    let original = InnerValue::Map(outer_map);

    let encoded = inner_to_json(&interner, &original).unwrap();
    let decoded = json_to_inner(&interner, &encoded).unwrap();
    assert_eq!(decoded, original);
}

#[test]
fn test_deeply_nested_roundtrip() {
    let interner = Interner::new();
    let mut current = InnerValue::Int(1);
    for _ in 0..10 {
        current = InnerValue::List(vec![current]);
    }
    let encoded = inner_to_json(&interner, &current).unwrap();
    let decoded = json_to_inner(&interner, &encoded).unwrap();
    assert_eq!(decoded, current);
}

// ---------------------------------------------------------------------------
// inner_to_json_value branches
// ---------------------------------------------------------------------------

#[test]
fn test_inner_to_json_value_scalar_types() {
    let interner = Interner::new();

    assert_eq!(
        inner_to_json_value(&InnerValue::Null, &interner).unwrap(),
        serde_json::Value::Null
    );
    assert_eq!(
        inner_to_json_value(&InnerValue::Bool(true), &interner).unwrap(),
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        inner_to_json_value(&InnerValue::Int(42), &interner).unwrap(),
        serde_json::Value::Number(42.into())
    );

    // Finite f64
    let f_val = inner_to_json_value(&InnerValue::F64(4.567), &interner).unwrap();
    assert!(f_val.is_number());

    // Non-finite f64 → string
    let inf_val = inner_to_json_value(&InnerValue::F64(f64::INFINITY), &interner).unwrap();
    assert!(inf_val.is_string());

    // Dec → string
    let dec_val =
        inner_to_json_value(&InnerValue::Dec(rust_decimal::Decimal::ONE), &interner).unwrap();
    assert!(dec_val.is_string());

    // Big → string
    let big_val =
        inner_to_json_value(&InnerValue::Big(num_bigint::BigInt::from(999)), &interner).unwrap();
    assert!(big_val.is_string());
}

#[test]
fn test_inner_to_json_value_set_and_bin() {
    let interner = Interner::new();

    let bin_val = inner_to_json_value(&InnerValue::Bin(vec![1, 2, 3]), &interner).unwrap();
    match bin_val {
        serde_json::Value::Array(arr) => {
            assert_eq!(arr.len(), 3);
        }
        _ => panic!("Bin should encode as JSON array"),
    }

    let mut set = new_set();
    set.insert(InnerValue::Int(1));
    let set_val = inner_to_json_value(&InnerValue::Set(set), &interner).unwrap();
    match set_val {
        serde_json::Value::Array(arr) => {
            assert_eq!(arr.len(), 1);
        }
        _ => panic!("Set should encode as JSON array"),
    }
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

#[test]
fn test_error_truncated_json() {
    let interner = Interner::new();
    // Start of a valid object but truncated
    let truncated = b"{\"key\":";
    let result = json_to_inner(&interner, truncated);
    assert!(result.is_err());
}

#[test]
fn test_error_empty_bytes() {
    let interner = Interner::new();
    let result = json_to_inner(&interner, b"");
    assert!(result.is_err());
}

#[test]
fn test_error_non_utf8_json() {
    let interner = Interner::new();
    let bad = &[0xFF, 0xFE, 0xFD];
    let result = json_to_inner(&interner, bad);
    assert!(result.is_err());
}

#[test]
fn test_json_value_to_inner_with_custom_intern_error() {
    use crate::codecs::CodecError;

    let fail_intern = |_key: &str| -> Result<InternerKey, CodecError> {
        Err(CodecError::Decode("intentional failure".to_string()))
    };
    let json = serde_json::json!({"key": "value"});
    let result = json_value_to_inner_with(&json, &fail_intern);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// json_value_to_inner_with: number edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_json_value_to_inner_with_large_uint_beyond_i64() {
    use crate::codecs::CodecError;
    use std::sync::atomic::{AtomicU64, Ordering};

    let counter = AtomicU64::new(1);
    let intern = |_key: &str| -> Result<InternerKey, CodecError> {
        Ok(InternerKey::new(counter.fetch_add(1, Ordering::SeqCst)))
    };

    // u64 that overflows i64 range but fits in f64 (serde_json limitation)
    let large: u64 = i64::MAX as u64 + 1; // 2^63
    let json = serde_json::json!({ "big": large });
    let inner = json_value_to_inner_with(&json, &intern).unwrap();
    match inner {
        InnerValue::Map(m) => {
            let val = m.values().next().unwrap();
            // serde_json may represent this as f64 or Str depending on precision
            match val {
                InnerValue::Str(s) => {
                    assert_eq!(s, &large.to_string());
                }
                InnerValue::F64(_) | InnerValue::Int(_) => {
                    // acceptable — serde_json stores large ints as f64
                }
                other => panic!("Expected Str/F64/Int, got {:?}", other),
            }
        }
        _ => panic!("Expected Map"),
    }
}

#[test]
fn test_json_value_to_inner_with_float_number() {
    use crate::codecs::CodecError;
    use std::sync::atomic::{AtomicU64, Ordering};

    let counter = AtomicU64::new(1);
    let intern = |_key: &str| -> Result<InternerKey, CodecError> {
        Ok(InternerKey::new(counter.fetch_add(1, Ordering::SeqCst)))
    };

    let json = serde_json::json!({ "f": 4.567 });
    let inner = json_value_to_inner_with(&json, &intern).unwrap();
    match inner {
        InnerValue::Map(m) => {
            let val = m.values().next().unwrap();
            match val {
                InnerValue::F64(f) => {
                    assert!((f - 4.567).abs() < 1e-10);
                }
                other => panic!("Expected F64, got {:?}", other),
            }
        }
        _ => panic!("Expected Map"),
    }
}
