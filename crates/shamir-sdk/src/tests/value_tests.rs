use crate::__rt::{block_on, decode_params, encode_value};
use crate::value::Value;

/// Round-trip a `Value::Int` through encode/decode and verify byte-identical
/// msgpack with `shamir_types::QueryValue`.
#[test]
fn int_roundtrip_matches_host() {
    let guest_val = Value::Int(42);
    let guest_bytes = encode_value(&guest_val);

    // Encode the same value with the host type.
    let host_val = shamir_types::types::value::QueryValue::Int(42);
    let host_bytes = host_val.to_bytes().unwrap();

    assert_eq!(
        guest_bytes, &*host_bytes,
        "guest and host msgpack must be byte-identical for Int(42)"
    );

    // Decode back.
    let decoded: Value = rmp_serde::from_slice(&guest_bytes).unwrap();
    assert_eq!(decoded, guest_val);
}

#[test]
fn bool_roundtrip_matches_host() {
    let guest_val = Value::Bool(true);
    let guest_bytes = encode_value(&guest_val);
    let host_val = shamir_types::types::value::QueryValue::Bool(true);
    let host_bytes = host_val.to_bytes().unwrap();
    assert_eq!(guest_bytes, &*host_bytes);
}

#[test]
fn f64_roundtrip_matches_host() {
    let guest_val = Value::F64(1.5);
    let guest_bytes = encode_value(&guest_val);
    let host_val = shamir_types::types::value::QueryValue::F64(1.5);
    let host_bytes = host_val.to_bytes().unwrap();
    assert_eq!(guest_bytes, &*host_bytes);
}

#[test]
fn str_roundtrip_matches_host() {
    let guest_val = Value::Str("hello".into());
    let guest_bytes = encode_value(&guest_val);
    let host_val = shamir_types::types::value::QueryValue::Str("hello".into());
    let host_bytes = host_val.to_bytes().unwrap();
    assert_eq!(guest_bytes, &*host_bytes);
}

#[test]
fn bin_roundtrip_matches_host() {
    let guest_val = Value::Bin(vec![0xDE, 0xAD, 0xBE, 0xEF]);
    let guest_bytes = encode_value(&guest_val);
    let host_val = shamir_types::types::value::QueryValue::Bin(vec![0xDE, 0xAD, 0xBE, 0xEF]);
    let host_bytes = host_val.to_bytes().unwrap();
    assert_eq!(guest_bytes, &*host_bytes);
}

#[test]
fn list_roundtrip_matches_host() {
    let guest_val = Value::List(vec![Value::Int(1), Value::Str("two".into())]);
    let guest_bytes = encode_value(&guest_val);
    let host_val = shamir_types::types::value::QueryValue::List(vec![
        shamir_types::types::value::QueryValue::Int(1),
        shamir_types::types::value::QueryValue::Str("two".into()),
    ]);
    let host_bytes = host_val.to_bytes().unwrap();
    assert_eq!(guest_bytes, &*host_bytes);
}

#[test]
fn map_roundtrip_matches_host() {
    use shamir_types::types::common::new_map;

    let guest_val = Value::Map(vec![
        ("n".into(), Value::Int(21)),
        ("s".into(), Value::Str("hello".into())),
    ]);
    let guest_bytes = encode_value(&guest_val);

    let mut host_map = new_map();
    host_map.insert("n".into(), shamir_types::types::value::QueryValue::Int(21));
    host_map.insert(
        "s".into(),
        shamir_types::types::value::QueryValue::Str("hello".into()),
    );
    let host_val = shamir_types::types::value::QueryValue::Map(host_map);
    let host_bytes = host_val.to_bytes().unwrap();

    assert_eq!(
        guest_bytes, &*host_bytes,
        "guest and host msgpack must be byte-identical for Map"
    );
}

#[test]
fn null_roundtrip_matches_host() {
    let guest_val = Value::Null;
    let guest_bytes = encode_value(&guest_val);
    let host_val = shamir_types::types::value::QueryValue::Null;
    let host_bytes = host_val.to_bytes().unwrap();
    assert_eq!(guest_bytes, &*host_bytes);
}

#[test]
fn decode_params_extracts_map() {
    use shamir_types::types::common::new_map;
    use shamir_types::types::value::QueryValue;

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
