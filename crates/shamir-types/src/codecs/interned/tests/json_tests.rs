use crate::codecs::interned::json::inner_value_to_query_value;
use crate::core::interner::Interner;
use crate::types::common::{new_map, new_set};
use crate::types::value::{InnerValue, QueryValue, Value};

// ---------------------------------------------------------------------------
// inner_value_to_query_value — direct, type-preserving path
//
// These tests verify that inner_value_to_query_value produces the correct
// QueryValue for each InnerValue variant without going through the (deleted)
// JSON codec.
// ---------------------------------------------------------------------------

#[test]
fn parity_null() {
    let interner = Interner::new();
    let v = InnerValue::Null;
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(direct, QueryValue::Null);
}

#[test]
fn parity_bool() {
    let interner = Interner::new();
    for b in [true, false] {
        let v = InnerValue::Bool(b);
        let direct = inner_value_to_query_value(&v, &interner).unwrap();
        assert_eq!(direct, QueryValue::Bool(b), "Bool({b})");
    }
}

#[test]
fn parity_int() {
    let interner = Interner::new();
    for i in [0i64, 1, -1, i64::MAX, i64::MIN] {
        let v = InnerValue::Int(i);
        let direct = inner_value_to_query_value(&v, &interner).unwrap();
        assert_eq!(direct, QueryValue::Int(i), "Int({i})");
    }
}

#[test]
fn parity_f64_finite() {
    let interner = Interner::new();
    for f in [0.0f64, 1.0, -1.0, 4.567, f64::MAX, f64::MIN_POSITIVE] {
        let v = InnerValue::F64(f);
        let direct = inner_value_to_query_value(&v, &interner).unwrap();
        assert_eq!(direct, QueryValue::F64(f), "F64({f})");
    }
}

#[test]
fn parity_str() {
    let interner = Interner::new();
    let v = InnerValue::Str("hello".to_string());
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(direct, QueryValue::Str("hello".to_string()));
}

#[test]
fn parity_list() {
    let interner = Interner::new();
    let v = InnerValue::List(vec![
        InnerValue::Int(1),
        InnerValue::Str("a".to_string()),
        InnerValue::Bool(false),
    ]);
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert!(
        matches!(direct, Value::List(_)),
        "List variant must be preserved, got {:?}",
        direct
    );
    if let Value::List(items) = &direct {
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], QueryValue::Int(1));
        assert_eq!(items[1], QueryValue::Str("a".to_string()));
        assert_eq!(items[2], QueryValue::Bool(false));
    }
}

// Dec: direct path preserves the Dec variant (the old JSON path was lossy → Str).
#[test]
fn parity_dec_direct_preserves_type() {
    let interner = Interner::new();
    let d = rust_decimal::Decimal::new(12345, 2); // 123.45
    let v = InnerValue::Dec(d);
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(direct, QueryValue::Dec(d), "direct path must preserve Dec");
}

// Big: direct path preserves the Big variant (the old JSON path was lossy → Str).
#[test]
fn parity_big_direct_preserves_type() {
    let interner = Interner::new();
    let b = num_bigint::BigInt::from(999_999_999_999i64);
    let v = InnerValue::Big(b.clone());
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(direct, QueryValue::Big(b), "direct path must preserve Big");
}

// Bin: direct path preserves the Bin variant (the old JSON path was lossy → List of Int).
#[test]
fn parity_bin_direct_preserves_type() {
    let interner = Interner::new();
    let bytes = vec![0xde_u8, 0xad, 0xbe, 0xef];
    let v = InnerValue::Bin(bytes.clone());
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert_eq!(
        direct,
        QueryValue::Bin(bytes),
        "direct path must preserve Bin"
    );
}

// Non-finite F64: direct path preserves the F64 variant (the old JSON path was lossy → Str).
#[test]
fn parity_f64_non_finite_direct_preserves_type() {
    let interner = Interner::new();
    for f in [f64::INFINITY, f64::NEG_INFINITY] {
        let v = InnerValue::F64(f);
        let direct = inner_value_to_query_value(&v, &interner).unwrap();
        assert!(
            matches!(direct, Value::F64(_)),
            "direct path must preserve F64 variant for non-finite F64({f}), got {:?}",
            direct
        );
    }
}

// Set: direct path preserves the Set variant (the old JSON path was lossy → List).
#[test]
fn parity_set_direct_preserves_type() {
    let interner = Interner::new();
    let mut s = new_set();
    s.insert(InnerValue::Int(1));
    s.insert(InnerValue::Int(2));
    let v = InnerValue::Set(s);
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert!(
        matches!(direct, Value::Set(_)),
        "direct path must preserve Set variant, got {:?}",
        direct
    );
}

// Map: inner_value_to_query_value produces QueryValue::Map with correct string keys/values.
#[test]
fn parity_map() {
    let interner = Interner::new();
    let k1 = interner.touch_ind("x").unwrap().into_key();
    let k2 = interner.touch_ind("y").unwrap().into_key();
    let mut m = new_map();
    m.insert(k1, InnerValue::Int(1));
    m.insert(k2, InnerValue::Str("two".to_string()));
    let v = InnerValue::Map(m);
    let direct = inner_value_to_query_value(&v, &interner).unwrap();
    assert!(
        matches!(direct, Value::Map(_)),
        "Map variant must be preserved, got {:?}",
        direct
    );
    if let Value::Map(qmap) = &direct {
        assert_eq!(qmap.get("x"), Some(&QueryValue::Int(1)));
        assert_eq!(qmap.get("y"), Some(&QueryValue::Str("two".to_string())));
    }
}
