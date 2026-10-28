//! Codec and HTTP envelope regression coverage consolidated from former top-level module.

use crate::error::ErrorKind;
use crate::Value;

#[test]
fn rv_sdk_b5acd67d_value_codec_roundtrips_nested_value() {
    let value = Value::List(vec![Value::Int(7), Value::Str("x".into())]);
    let encoded = crate::__rt::encode_value(&value).expect("encode");
    assert_eq!(
        crate::__rt::decode_value_bytes(&encoded).expect("decode"),
        value
    );
}

#[test]
fn rv_sdk_6d2497ca_malformed_host_response_is_codec() {
    assert_eq!(
        crate::__rt::decode_value_bytes(&[0x81, 0xa1])
            .unwrap_err()
            .kind(),
        ErrorKind::Codec
    );
}

#[test]
fn rv_sdk_6d2497ca_malformed_http_envelope_is_codec() {
    for raw in [Value::Int(1), Value::List(vec![Value::Int(1), Value::Null])] {
        assert_eq!(
            crate::http::decode_fetch_envelope(&raw).unwrap_err().kind(),
            ErrorKind::Codec
        );
    }
    let bad_response = Value::List(vec![
        Value::Bool(true),
        Value::Map(vec![
            ("status".into(), Value::Int(200)),
            ("body".into(), Value::Str("bad".into())),
        ]),
    ]);
    assert_eq!(
        crate::http::decode_fetch_envelope(&bad_response)
            .unwrap_err()
            .kind(),
        ErrorKind::Codec
    );
    let denial = Value::List(vec![Value::Bool(false), Value::Str("denied".into())]);
    assert_eq!(
        crate::http::decode_fetch_envelope(&denial)
            .unwrap_err()
            .kind(),
        ErrorKind::User
    );
}
