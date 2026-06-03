use crate::codecs::interned::{inner_to_msgpack, msgpack_to_inner};
use crate::core::interner::Interner;
use crate::types::common::{new_map, new_set};
use crate::types::value::InnerValue;
use std::str::FromStr;

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
        (
            rmpv::Value::from("tags"),
            rmpv::Value::Array(vec![rmpv::Value::from("rust"), rmpv::Value::from("db")]),
        ),
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
        (
            rmpv::Value::from("user"),
            rmpv::Value::Map(vec![
                (rmpv::Value::from("name"), rmpv::Value::from("Bob")),
                (
                    rmpv::Value::from("prefs"),
                    rmpv::Value::Map(vec![(
                        rmpv::Value::from("theme"),
                        rmpv::Value::from("dark"),
                    )]),
                ),
            ]),
        ),
        (
            rmpv::Value::from("items"),
            rmpv::Value::Array(vec![
                rmpv::Value::from(1i64),
                rmpv::Value::from(2i64),
                rmpv::Value::from(3i64),
            ]),
        ),
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
        (
            rmpv::Value::from("nested"),
            rmpv::Value::Map(vec![(rmpv::Value::from("key"), rmpv::Value::from("other"))]),
        ),
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

    let test_data = rmpv::Value::Map(vec![(
        rmpv::Value::from("data"),
        rmpv::Value::Binary(vec![1, 2, 3, 4, 5]),
    )]);

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
        (rmpv::Value::from("float_val"), rmpv::Value::from(3.9)),
        (rmpv::Value::from("string_val"), rmpv::Value::from("hello")),
        (
            rmpv::Value::from("binary_val"),
            rmpv::Value::Binary(vec![1, 2, 3]),
        ),
        (
            rmpv::Value::from("array_val"),
            rmpv::Value::Array(vec![rmpv::Value::from(1i64), rmpv::Value::from(2i64)]),
        ),
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
fn test_null_value() {
    let interner = Interner::new();

    let inner = InnerValue::Null;
    let msgpack = inner_to_msgpack(&interner, &inner).unwrap();

    let decoded: rmpv::Value = rmpv::decode::read_value(&mut &*msgpack).unwrap();
    assert_eq!(decoded, rmpv::Value::Nil);
}

#[test]
fn test_null_roundtrip() {
    let interner = Interner::new();

    let original = InnerValue::Null;
    let msgpack = inner_to_msgpack(&interner, &original).unwrap();
    let decoded = msgpack_to_inner(&interner, &msgpack).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_null_in_map_roundtrip() {
    let interner = Interner::new();

    let test_data = rmpv::Value::Map(vec![(rmpv::Value::from("field"), rmpv::Value::Nil)]);

    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &test_data).unwrap();

    let inner = msgpack_to_inner(&interner, &buf).unwrap();
    let result = inner_to_msgpack(&interner, &inner).unwrap();

    assert_eq!(buf, result);

    match inner {
        InnerValue::Map(m) => {
            let key = interner.touch_ind("field").unwrap().key().clone();
            assert_eq!(m.get(&key), Some(&InnerValue::Null));
        }
        _ => panic!("Expected Map"),
    }
}

#[test]
fn test_large_unsigned_int() {
    let interner = Interner::new();

    // Large unsigned integer that doesn't fit in i64
    let large_u64: u64 = i64::MAX as u64 + 1;
    let test_data = rmpv::Value::Map(vec![(
        rmpv::Value::from("large"),
        rmpv::Value::from(large_u64),
    )]);

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

// ---------------------------------------------------------------------------
// Scalar round-trips
// ---------------------------------------------------------------------------

#[test]
fn test_bool_roundtrip() {
    let interner = Interner::new();
    for b in [true, false] {
        let original = InnerValue::Bool(b);
        let encoded = inner_to_msgpack(&interner, &original).unwrap();
        let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
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
        let encoded = inner_to_msgpack(&interner, &original).unwrap();
        let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
        assert_eq!(decoded, original, "int roundtrip failed for {}", v);
    }
}

#[test]
fn test_f64_roundtrip() {
    let interner = Interner::new();
    for &v in &[
        0.0f64,
        -0.0,
        1.5,
        -99.999,
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        let original = InnerValue::F64(v);
        let encoded = inner_to_msgpack(&interner, &original).unwrap();
        let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
        assert_eq!(decoded, original, "f64 roundtrip for {}", v);
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
        "x".repeat(10_000),
    ];
    for s in cases {
        let original = InnerValue::Str(s.clone());
        let encoded = inner_to_msgpack(&interner, &original).unwrap();
        let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
        assert_eq!(decoded, original, "string roundtrip for len={}", s.len());
    }
}

#[test]
fn test_bin_roundtrip() {
    let interner = Interner::new();
    let cases: Vec<Vec<u8>> = vec![
        vec![],
        vec![0],
        vec![255],
        vec![0, 1, 2, 3, 4, 5],
        (0..=255).collect(),
    ];
    for b in cases {
        let original = InnerValue::Bin(b.clone());
        let encoded = inner_to_msgpack(&interner, &original).unwrap();
        let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
        assert_eq!(decoded, original, "bin roundtrip for len={}", b.len());
    }
}

#[test]
fn test_list_roundtrip_empty_and_nested() {
    let interner = Interner::new();
    let empty = InnerValue::List(vec![]);
    let encoded = inner_to_msgpack(&interner, &empty).unwrap();
    let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
    assert_eq!(decoded, empty);

    let nested = InnerValue::List(vec![
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
        InnerValue::List(vec![]),
        InnerValue::Null,
    ]);
    let encoded = inner_to_msgpack(&interner, &nested).unwrap();
    let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
    assert_eq!(decoded, nested);
}

#[test]
fn test_set_roundtrip() {
    let interner = Interner::new();
    let mut set = new_set();
    set.insert(InnerValue::Int(1));
    set.insert(InnerValue::Str("hello".to_string()));
    let original = InnerValue::Set(set);
    let encoded = inner_to_msgpack(&interner, &original).unwrap();
    let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
    // MessagePack encodes set as array → decoded as List
    match decoded {
        InnerValue::List(arr) => {
            assert_eq!(arr.len(), 2);
        }
        other => panic!("Expected List (from Set), got {:?}", other),
    }
}

#[test]
fn test_map_roundtrip_varieties() {
    let interner = Interner::new();
    // Empty
    let empty = InnerValue::Map(new_map());
    let encoded = inner_to_msgpack(&interner, &empty).unwrap();
    let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
    assert_eq!(decoded, empty);

    // With interned keys
    let k1 = interner.touch_ind("a").unwrap().into_key();
    let k2 = interner.touch_ind("b").unwrap().into_key();
    let mut m = new_map();
    m.insert(k1, InnerValue::Int(1));
    m.insert(k2, InnerValue::Str("two".to_string()));
    let original = InnerValue::Map(m);

    let encoded = inner_to_msgpack(&interner, &original).unwrap();
    let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
    assert_eq!(decoded, original);
}

#[test]
fn test_deeply_nested_roundtrip() {
    let interner = Interner::new();
    let mut current = InnerValue::Int(1);
    for _ in 0..10 {
        current = InnerValue::List(vec![current]);
    }
    let encoded = inner_to_msgpack(&interner, &current).unwrap();
    let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
    assert_eq!(decoded, current);
}

#[test]
fn test_dec_and_big_roundtrip_as_string() {
    let interner = Interner::new();
    // Dec serialises as string; after round-trip it comes back as Str
    let dec_val = InnerValue::Dec(rust_decimal::Decimal::from_str("123.456").unwrap());
    let encoded = inner_to_msgpack(&interner, &dec_val).unwrap();
    let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
    match decoded {
        InnerValue::Str(s) => assert_eq!(s, "123.456"),
        other => panic!("Expected Str from Dec, got {:?}", other),
    }

    let big_val = InnerValue::Big(num_bigint::BigInt::from(999));
    let encoded = inner_to_msgpack(&interner, &big_val).unwrap();
    let decoded = msgpack_to_inner(&interner, &encoded).unwrap();
    match decoded {
        InnerValue::Str(s) => assert_eq!(s, "999"),
        other => panic!("Expected Str from Big, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// rmpv value edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_msgpack_f32_coerced_to_f64() {
    let interner = Interner::new();
    let test_data = rmpv::Value::Map(vec![(rmpv::Value::from("f"), rmpv::Value::F32(4.567))]);
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &test_data).unwrap();
    let inner = msgpack_to_inner(&interner, &buf).unwrap();
    match inner {
        InnerValue::Map(m) => {
            let k = interner.touch_ind("f").unwrap().into_key();
            match m.get(&k) {
                Some(InnerValue::F64(f)) => {
                    assert!((f - 4.567f32 as f64).abs() < 1e-5);
                }
                other => panic!("Expected F64, got {:?}", other),
            }
        }
        _ => panic!("Expected Map"),
    }
}

#[test]
fn test_msgpack_ext_type_stored_as_bin() {
    let interner = Interner::new();
    let ext = rmpv::Value::Ext(42, vec![1, 2, 3]);
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &ext).unwrap();
    let inner = msgpack_to_inner(&interner, &buf).unwrap();
    match inner {
        InnerValue::Bin(b) => assert_eq!(b, vec![1, 2, 3]),
        other => panic!("Expected Bin from Ext, got {:?}", other),
    }
}

#[test]
fn test_msgpack_non_string_map_key_error() {
    let interner = Interner::new();
    let test_data = rmpv::Value::Map(vec![(
        rmpv::Value::from(42i64), // Non-string key
        rmpv::Value::from("value"),
    )]);
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &test_data).unwrap();
    let result = msgpack_to_inner(&interner, &buf);
    assert!(result.is_err());
    match result.unwrap_err() {
        crate::codecs::CodecError::Decode(msg) => {
            assert!(msg.contains("Map keys must be strings"));
        }
        other => panic!("Expected Decode error, got {:?}", other),
    }
}

#[test]
fn test_msgpack_large_uint_as_string() {
    let interner = Interner::new();
    // u64 > i64::MAX → stored as String
    let large = i64::MAX as u64 + 1;
    let test_data = rmpv::Value::Map(vec![(rmpv::Value::from("v"), rmpv::Value::from(large))]);
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &test_data).unwrap();
    let inner = msgpack_to_inner(&interner, &buf).unwrap();
    match inner {
        InnerValue::Map(m) => {
            let k = interner.touch_ind("v").unwrap().into_key();
            match m.get(&k) {
                Some(InnerValue::Str(s)) => assert_eq!(s, &large.to_string()),
                other => panic!("Expected Str, got {:?}", other),
            }
        }
        _ => panic!("Expected Map"),
    }
}

#[test]
fn test_msgpack_uint_fits_in_i64() {
    let interner = Interner::new();
    // u64 that fits in i64
    let val = 42u64;
    let test_data = rmpv::Value::Map(vec![(rmpv::Value::from("v"), rmpv::Value::from(val))]);
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &test_data).unwrap();
    let inner = msgpack_to_inner(&interner, &buf).unwrap();
    match inner {
        InnerValue::Map(m) => {
            let k = interner.touch_ind("v").unwrap().into_key();
            assert_eq!(m.get(&k), Some(&InnerValue::Int(42)));
        }
        _ => panic!("Expected Map"),
    }
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

#[test]
fn test_error_completely_invalid_bytes() {
    let interner = Interner::new();
    // 0xC1 is reserved in MessagePack but rmpv decodes it as a value;
    // use truly truncated data instead.
    let result = msgpack_to_inner(&interner, &[0x92, 0x01]);
    assert!(result.is_err());
}

#[test]
fn test_error_empty_bytes() {
    let interner = Interner::new();
    let result = msgpack_to_inner(&interner, &[]);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Depth cap (audit fix 3)
// ---------------------------------------------------------------------------

#[test]
fn test_deeply_nested_msgpack_rejected() {
    let interner = Interner::new();

    // Build a 200-level nested array via rmpv
    let mut val = rmpv::Value::from(42i64);
    for _ in 0..200 {
        val = rmpv::Value::Array(vec![val]);
    }

    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &val).unwrap();

    let result = msgpack_to_inner(&interner, &buf);
    assert!(result.is_err(), "deeply-nested msgpack should be rejected");
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("nesting depth"),
        "expected depth error, got: {msg}"
    );
}

#[test]
fn test_normal_nesting_roundtrip_still_works() {
    let interner = Interner::new();

    // 10-level nesting — well within the 128 cap
    let mut val = rmpv::Value::from(1i64);
    for _ in 0..10 {
        val = rmpv::Value::Array(vec![val]);
    }

    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &val).unwrap();

    let inner = msgpack_to_inner(&interner, &buf).unwrap();
    let result = inner_to_msgpack(&interner, &inner).unwrap();
    assert_eq!(buf, result);
}
