use crate::codecs::interned::json::{inner_to_json, inner_to_json_value, json_value_to_inner_with};
use crate::core::interner::Interner;
use crate::core::interner::InternerKey;
use crate::types::common::{new_map, new_set};
use crate::types::value::InnerValue;

#[test]
fn test_inner_to_json_simple() {
    let interner = Interner::new();

    let name_key = interner.touch_ind("name").unwrap().into_key();
    let age_key = interner.touch_ind("age").unwrap().into_key();

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
