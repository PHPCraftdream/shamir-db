use crate::codecs::interned::json::{
    inner_to_json, inner_to_json_value, inner_value_to_query_value, json_value_to_inner_with,
};
use crate::core::interner::Interner;
use crate::core::interner::InternerKey;
use crate::types::common::{new_map, new_set};
use crate::types::value::{InnerValue, QueryValue, Value};

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

// ---------------------------------------------------------------------------
// Parity: inner_value_to_query_value vs QueryValue::from(inner_to_json_value)
//
// The computed-field write path previously did a two-step hop:
//   inner_to_json_value(x, i) → json::Value → QueryValue::from(jv)
// and now does:
//   inner_value_to_query_value(x, i) → QueryValue  (direct, type-preserving)
//
// For Null/Bool/Int/Str/finite-F64/List the two paths produce the same
// QueryValue, so these assert equality.
//
// For Dec, Big, Bin, and non-finite F64 the old JSON round-trip was LOSSY:
//   - Dec / Big   → json::Value::String → QueryValue::Str  (type erased)
//   - Bin         → json::Value::Array of byte ints → QueryValue::List of Int (type changed)
//   - F64 inf/nan → json::Value::String → QueryValue::Str  (type erased)
// The direct path preserves the original variant. These tests assert that
// the direct path is type-preserving, and document the old lossy output as
// a comment so the divergence is explicit.
// ---------------------------------------------------------------------------

#[test]
fn parity_null() {
    let interner = Interner::new();
    let v = InnerValue::Null;
    let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(via_json, direct);
}

#[test]
fn parity_bool() {
    let interner = Interner::new();
    for b in [true, false] {
        let v = InnerValue::Bool(b);
        let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
        let direct = inner_value_to_query_value(&v, &interner).unwrap();
        assert_eq!(via_json, direct, "Bool({b})");
    }
}

#[test]
fn parity_int() {
    let interner = Interner::new();
    for i in [0i64, 1, -1, i64::MAX, i64::MIN] {
        let v = InnerValue::Int(i);
        let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
        let direct = inner_value_to_query_value(&v, &interner).unwrap();
        assert_eq!(via_json, direct, "Int({i})");
    }
}

#[test]
fn parity_f64_finite() {
    let interner = Interner::new();
    for f in [0.0f64, 1.0, -1.0, 4.567, f64::MAX, f64::MIN_POSITIVE] {
        let v = InnerValue::F64(f);
        let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
        let direct = inner_value_to_query_value(&v, &interner).unwrap();
        assert_eq!(via_json, direct, "F64({f})");
    }
}

#[test]
fn parity_str() {
    let interner = Interner::new();
    let v = InnerValue::Str("hello".to_string());
    let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(via_json, direct);
}

#[test]
fn parity_list() {
    let interner = Interner::new();
    let v = InnerValue::List(vec![
        InnerValue::Int(1),
        InnerValue::Str("a".to_string()),
        InnerValue::Bool(false),
    ]);
    let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(via_json, direct);
}

// Dec: old path → QueryValue::Str (JSON string); direct → QueryValue::Dec (type-preserving).
#[test]
fn parity_dec_direct_preserves_type() {
    let interner = Interner::new();
    let d = rust_decimal::Decimal::new(12345, 2); // 123.45
    let v = InnerValue::Dec(d);

    // Old lossy path: Dec becomes a JSON string, then QueryValue::Str.
    let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
    assert!(
        matches!(via_json, Value::Str(_)),
        "old path must produce Str, got {:?}",
        via_json
    );

    // Direct path: type-preserving.
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(direct, QueryValue::Dec(d), "direct path must preserve Dec");
}

// Big: old path → QueryValue::Str; direct → QueryValue::Big (type-preserving).
#[test]
fn parity_big_direct_preserves_type() {
    let interner = Interner::new();
    let b = num_bigint::BigInt::from(999_999_999_999i64);
    let v = InnerValue::Big(b.clone());

    let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
    assert!(
        matches!(via_json, Value::Str(_)),
        "old path must produce Str, got {:?}",
        via_json
    );

    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(direct, QueryValue::Big(b), "direct path must preserve Big");
}

// Bin: old path → QueryValue::List of Int (byte array encoded in JSON);
//      direct → QueryValue::Bin (type-preserving).
#[test]
fn parity_bin_direct_preserves_type() {
    let interner = Interner::new();
    let bytes = vec![0xde_u8, 0xad, 0xbe, 0xef];
    let v = InnerValue::Bin(bytes.clone());

    let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
    assert!(
        matches!(via_json, Value::List(_)),
        "old path must produce List (byte-array in JSON), got {:?}",
        via_json
    );

    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(
        direct,
        QueryValue::Bin(bytes),
        "direct path must preserve Bin"
    );
}

// Non-finite F64: old path → QueryValue::Str; direct → QueryValue::F64 (type-preserving).
#[test]
fn parity_f64_non_finite_direct_preserves_type() {
    let interner = Interner::new();
    for f in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
        let v = InnerValue::F64(f);

        let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
        assert!(
            matches!(via_json, Value::Str(_)),
            "old path must produce Str for non-finite F64({f}), got {:?}",
            via_json
        );

        let direct = inner_value_to_query_value(&v, &interner).unwrap();
        assert!(
            matches!(direct, Value::F64(_)),
            "direct path must preserve F64 variant for non-finite F64({f}), got {:?}",
            direct
        );
    }
}

// Set: old path encodes as JSON array → QueryValue::List;
//      direct → QueryValue::Set (type-preserving).
#[test]
fn parity_set_direct_preserves_type() {
    let interner = Interner::new();
    let mut s = new_set();
    s.insert(InnerValue::Int(1));
    s.insert(InnerValue::Int(2));
    let v = InnerValue::Set(s);

    let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
    assert!(
        matches!(via_json, Value::List(_)),
        "old path must produce List (Set encodes as JSON array), got {:?}",
        via_json
    );

    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert!(
        matches!(direct, Value::Set(_)),
        "direct path must preserve Set variant, got {:?}",
        direct
    );
}

// Map: both paths produce QueryValue::Map with identical string keys and values
//      (for string-keyed maps with no lossy inner types).
#[test]
fn parity_map() {
    let interner = Interner::new();
    let k1 = interner.touch_ind("x").unwrap().into_key();
    let k2 = interner.touch_ind("y").unwrap().into_key();
    let mut m = new_map();
    m.insert(k1, InnerValue::Int(1));
    m.insert(k2, InnerValue::Str("two".to_string()));
    let v = InnerValue::Map(m);

    let via_json = QueryValue::from(inner_to_json_value(&v, &interner).unwrap());
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(via_json, direct);
}
