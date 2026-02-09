use crate::codecs::interned_msgpack::{msgpack_to_inner, inner_to_msgpack};
use crate::core::interner::Interner;
use crate::types::value::InnerValue;
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
    let decoded: rmpv::Value = rmpv::decode::read_value(&mut &*msgpack).unwrap();

    match decoded {
        rmpv::Value::Map(m) => {
            let mut found_name = false;
            let mut found_age = false;

            for (key, val) in &m {
                if let rmpv::Value::String(k) = key {
                    if let Some("name") = k.as_str() {
                        found_name = true;
                        assert_eq!(val, &rmpv::Value::String("Alice".into()));
                    } else if let Some("age") = k.as_str() {
                        found_age = true;
                        assert_eq!(val, &rmpv::Value::Integer(30.into()));
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
        (rmpv::Value::from("null_val"), rmpv::Value::Nil),
        (rmpv::Value::from("bool_true"), rmpv::Value::Boolean(true)),
        (rmpv::Value::from("bool_false"), rmpv::Value::Boolean(false)),
        (rmpv::Value::from("int_val"), rmpv::Value::from(-42i64)),
        (rmpv::Value::from("uint_val"), rmpv::Value::from(42u32)),
        (rmpv::Value::from("float_val"), rmpv::Value::from(3.14f64)),
        (rmpv::Value::from("string_val"), rmpv::Value::from("hello")),
        (rmpv::Value::from("binary_val"), rmpv::Value::Binary(vec![1, 2, 3])),
        (rmpv::Value::from("array_val"), rmpv::Value::Array(vec![
            rmpv::Value::from(1i64),
            rmpv::Value::from(2i64),
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

    let decoded: rmpv::Value = rmpv::decode::read_value(&mut &*msgpack).unwrap();
    assert_eq!(decoded, rmpv::Value::Nil);
}

#[test]
fn test_large_unsigned_int() {
    let interner = Interner::new();

    // Large unsigned integer that doesn't fit in i64
    let large_u64: u64 = i64::MAX as u64 + 1;
    let test_data = rmpv::Value::Map(vec![
        (rmpv::Value::from("large"), rmpv::Value::from(large_u64)),
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
