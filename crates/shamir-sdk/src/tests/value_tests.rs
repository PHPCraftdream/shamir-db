use crate::__rt::{block_on, decode_params, encode_value};
use crate::value::Value;
use shamir_types::types::value::QueryValue;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Assert guest→host and host→guest byte-identity for a matched pair.
///
/// 1. guest `Value` → encode → bytes must equal host `QueryValue` → encode.
/// 2. host bytes → decode as guest `Value` must equal the original guest value.
/// 3. guest bytes → decode as host `QueryValue` must equal the original host value.
fn assert_bidirectional(guest: &Value, host: &QueryValue) {
    let guest_bytes = encode_value(guest);
    let host_bytes = host.to_bytes().unwrap();

    assert_eq!(
        guest_bytes, &*host_bytes,
        "guest→bytes and host→bytes must be identical"
    );

    // host bytes → guest Value
    let decoded_guest: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    assert_eq!(
        &decoded_guest, guest,
        "host bytes decoded as guest Value must match original"
    );

    // guest bytes → host QueryValue
    let decoded_host: QueryValue = rmp_serde::from_slice(&guest_bytes).unwrap();
    assert_eq!(
        &decoded_host, host,
        "guest bytes decoded as host QueryValue must match original"
    );
}

// ===========================================================================
// Bidirectional wire-conformance: every shared variant
// ===========================================================================

#[test]
fn null_bidirectional() {
    assert_bidirectional(&Value::Null, &QueryValue::Null);
}

#[test]
fn bool_true_bidirectional() {
    assert_bidirectional(&Value::Bool(true), &QueryValue::Bool(true));
}

#[test]
fn bool_false_bidirectional() {
    assert_bidirectional(&Value::Bool(false), &QueryValue::Bool(false));
}

#[test]
fn int_positive_bidirectional() {
    assert_bidirectional(&Value::Int(42), &QueryValue::Int(42));
}

#[test]
fn int_zero_bidirectional() {
    assert_bidirectional(&Value::Int(0), &QueryValue::Int(0));
}

#[test]
fn int_negative_bidirectional() {
    assert_bidirectional(&Value::Int(-999), &QueryValue::Int(-999));
}

#[test]
fn int_max_bidirectional() {
    assert_bidirectional(&Value::Int(i64::MAX), &QueryValue::Int(i64::MAX));
}

#[test]
fn int_min_bidirectional() {
    assert_bidirectional(&Value::Int(i64::MIN), &QueryValue::Int(i64::MIN));
}

#[test]
fn f64_normal_bidirectional() {
    assert_bidirectional(&Value::F64(1.5), &QueryValue::F64(1.5));
}

#[test]
fn f64_zero_bidirectional() {
    assert_bidirectional(&Value::F64(0.0), &QueryValue::F64(0.0));
}

#[test]
fn f64_negative_bidirectional() {
    assert_bidirectional(&Value::F64(-273.15), &QueryValue::F64(-273.15));
}

#[test]
fn str_empty_bidirectional() {
    assert_bidirectional(&Value::Str(String::new()), &QueryValue::Str(String::new()));
}

#[test]
fn str_ascii_bidirectional() {
    assert_bidirectional(
        &Value::Str("hello".into()),
        &QueryValue::Str("hello".into()),
    );
}

#[test]
fn str_unicode_bidirectional() {
    assert_bidirectional(
        &Value::Str("\u{05E9}\u{05DC}\u{05D5}\u{05DD}".into()),
        &QueryValue::Str("\u{05E9}\u{05DC}\u{05D5}\u{05DD}".into()),
    );
}

#[test]
fn bin_bidirectional() {
    assert_bidirectional(
        &Value::Bin(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        &QueryValue::Bin(vec![0xDE, 0xAD, 0xBE, 0xEF]),
    );
}

#[test]
fn bin_empty_bidirectional() {
    assert_bidirectional(&Value::Bin(vec![]), &QueryValue::Bin(vec![]));
}

#[test]
fn list_bidirectional() {
    assert_bidirectional(
        &Value::List(vec![Value::Int(1), Value::Str("two".into())]),
        &QueryValue::List(vec![QueryValue::Int(1), QueryValue::Str("two".into())]),
    );
}

#[test]
fn list_empty_bidirectional() {
    assert_bidirectional(&Value::List(vec![]), &QueryValue::List(vec![]));
}

#[test]
fn map_bidirectional() {
    use shamir_types::types::common::new_map;

    let guest = Value::Map(vec![
        ("n".into(), Value::Int(21)),
        ("s".into(), Value::Str("hello".into())),
    ]);

    let mut host_map = new_map();
    host_map.insert("n".into(), QueryValue::Int(21));
    host_map.insert("s".into(), QueryValue::Str("hello".into()));
    let host = QueryValue::Map(host_map);

    let guest_bytes = encode_value(&guest);
    let host_bytes = host.to_bytes().unwrap();

    assert_eq!(
        guest_bytes, &*host_bytes,
        "guest→bytes and host→bytes must be identical for Map"
    );

    // host bytes → guest
    let decoded_guest: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    assert_eq!(decoded_guest, guest);

    // guest bytes → host
    let decoded_host: QueryValue = rmp_serde::from_slice(&guest_bytes).unwrap();
    assert_eq!(decoded_host, host);
}

#[test]
fn map_empty_bidirectional() {
    use shamir_types::types::common::new_map;

    let guest = Value::Map(vec![]);
    let host = QueryValue::Map(new_map());

    let guest_bytes = encode_value(&guest);
    let host_bytes = host.to_bytes().unwrap();
    assert_eq!(guest_bytes, &*host_bytes);
}

// ===========================================================================
// Nested values
// ===========================================================================

#[test]
fn nested_list_in_map_bidirectional() {
    use shamir_types::types::common::new_map;

    let guest = Value::Map(vec![(
        "items".into(),
        Value::List(vec![
            Value::Int(1),
            Value::Bool(true),
            Value::Str("three".into()),
        ]),
    )]);

    let mut host_map = new_map();
    host_map.insert(
        "items".into(),
        QueryValue::List(vec![
            QueryValue::Int(1),
            QueryValue::Bool(true),
            QueryValue::Str("three".into()),
        ]),
    );
    let host = QueryValue::Map(host_map);

    let guest_bytes = encode_value(&guest);
    let host_bytes = host.to_bytes().unwrap();
    assert_eq!(guest_bytes, &*host_bytes);

    let decoded_guest: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    assert_eq!(decoded_guest, guest);
}

#[test]
fn nested_map_in_list_bidirectional() {
    use shamir_types::types::common::new_map;

    let guest = Value::List(vec![
        Value::Map(vec![("a".into(), Value::Int(1))]),
        Value::Null,
    ]);

    let mut inner = new_map();
    inner.insert("a".into(), QueryValue::Int(1));
    let host = QueryValue::List(vec![QueryValue::Map(inner), QueryValue::Null]);

    let guest_bytes = encode_value(&guest);
    let host_bytes = host.to_bytes().unwrap();
    assert_eq!(guest_bytes, &*host_bytes);

    let decoded_guest: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    assert_eq!(decoded_guest, guest);
}

#[test]
fn deeply_nested_bidirectional() {
    use shamir_types::types::common::new_map;

    // guest: {"a": [{"b": 42}]}
    let guest = Value::Map(vec![(
        "a".into(),
        Value::List(vec![Value::Map(vec![("b".into(), Value::Int(42))])]),
    )]);

    let mut inner = new_map();
    inner.insert("b".into(), QueryValue::Int(42));
    let mut outer = new_map();
    outer.insert("a".into(), QueryValue::List(vec![QueryValue::Map(inner)]));
    let host = QueryValue::Map(outer);

    let guest_bytes = encode_value(&guest);
    let host_bytes = host.to_bytes().unwrap();
    assert_eq!(guest_bytes, &*host_bytes);

    let decoded_guest: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    assert_eq!(decoded_guest, guest);

    let decoded_host: QueryValue = rmp_serde::from_slice(&guest_bytes).unwrap();
    assert_eq!(decoded_host, host);
}

// ===========================================================================
// Lossy-but-stable: host-only variants decoded into guest equivalents
// ===========================================================================

/// Host `QueryValue::Dec(Decimal)` serializes as a string on the wire.
/// Guest decodes it as `Value::Str` — lossy but stable.
#[test]
fn lossy_dec_to_str() {
    use rust_decimal::Decimal;
    use std::str::FromStr;

    let dec = Decimal::from_str("123.456").unwrap();
    let host = QueryValue::Dec(dec);
    let host_bytes = host.to_bytes().unwrap();

    // Guest sees it as Str("123.456")
    let decoded: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    assert_eq!(
        decoded,
        Value::Str("123.456".into()),
        "host Dec must decode as guest Str"
    );
}

/// Host `QueryValue::Dec` with a whole number (no fractional part).
#[test]
fn lossy_dec_whole_to_str() {
    use rust_decimal::Decimal;

    let dec = Decimal::from(1000i64);
    let host = QueryValue::Dec(dec);
    let host_bytes = host.to_bytes().unwrap();

    let decoded: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    assert_eq!(decoded, Value::Str("1000".into()));
}

/// Host `QueryValue::Big(BigInt)` serializes as a string on the wire.
/// Guest decodes it as `Value::Str`.
#[test]
fn lossy_big_to_str() {
    use num_bigint::BigInt;
    use std::str::FromStr;

    let big = BigInt::from_str("999999999999999999999999999999").unwrap();
    let expected_str = "999999999999999999999999999999";
    let host = QueryValue::Big(big);
    let host_bytes = host.to_bytes().unwrap();

    let decoded: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    assert_eq!(
        decoded,
        Value::Str(expected_str.into()),
        "host Big must decode as guest Str"
    );
}

/// Host `QueryValue::Big` with a negative value.
#[test]
fn lossy_big_negative_to_str() {
    use num_bigint::BigInt;

    let big = BigInt::from(-42i64);
    let host = QueryValue::Big(big);
    let host_bytes = host.to_bytes().unwrap();

    let decoded: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    assert_eq!(decoded, Value::Str("-42".into()));
}

/// Host `QueryValue::Set(TSet)` serializes as a sequence on the wire.
/// Guest decodes it as `Value::List`.
#[test]
fn lossy_set_to_list() {
    use shamir_types::types::common::new_set;

    let mut set = new_set();
    set.insert(QueryValue::Int(1));
    set.insert(QueryValue::Int(2));
    set.insert(QueryValue::Int(3));
    let host = QueryValue::Set(set);
    let host_bytes = host.to_bytes().unwrap();

    let decoded: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    // IndexSet preserves insertion order, so the list order is deterministic.
    assert_eq!(
        decoded,
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        "host Set must decode as guest List"
    );
}

/// Host `QueryValue::Set` with mixed types.
#[test]
fn lossy_set_mixed_to_list() {
    use shamir_types::types::common::new_set;

    let mut set = new_set();
    set.insert(QueryValue::Str("hello".into()));
    set.insert(QueryValue::Bool(true));
    let host = QueryValue::Set(set);
    let host_bytes = host.to_bytes().unwrap();

    let decoded: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    assert_eq!(
        decoded,
        Value::List(vec![Value::Str("hello".into()), Value::Bool(true)]),
        "host Set (mixed) must decode as guest List"
    );
}

/// Empty host `Set` → empty guest `List`.
#[test]
fn lossy_set_empty_to_list() {
    use shamir_types::types::common::new_set;

    let set = new_set();
    let host = QueryValue::Set(set);
    let host_bytes = host.to_bytes().unwrap();

    let decoded: Value = rmp_serde::from_slice(&host_bytes).unwrap();
    assert_eq!(decoded, Value::List(vec![]));
}

// ===========================================================================
// Existing tests (retained)
// ===========================================================================

#[test]
fn decode_params_extracts_map() {
    use shamir_types::types::common::new_map;

    let mut host_map = new_map();
    host_map.insert("n".into(), QueryValue::Int(21));
    host_map.insert("msg".into(), QueryValue::Str("hi".into()));
    let encoded = QueryValue::Map(host_map).to_bytes().unwrap();

    let params = decode_params(&encoded);
    assert_eq!(params.i64("n").unwrap(), 21);
    assert_eq!(params.str("msg").unwrap(), "hi");
}

#[test]
fn block_on_resolves_immediately() {
    let result = block_on(async { 42 });
    assert_eq!(result, 42);
}
