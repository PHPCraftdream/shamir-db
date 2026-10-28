use crate::__rt;
use crate::error::ErrorKind;
use crate::Value;

#[test]
fn rv_sdk_8190492d_response_buffer_reclaimed_after_decode() {
    assert_eq!(__rt::live_response_bytes(), 0);
    let packed = rmp_serde::to_vec(&Value::Str("ok".into())).expect("encode");
    let len = packed.len();
    let mut bytes = vec![0u8; len];
    bytes.copy_from_slice(&packed);
    let ptr = bytes.as_mut_ptr();
    let len = bytes.len();
    assert_eq!(
        bytes.capacity(),
        len,
        "exact-capacity host response allocation"
    );
    core::mem::forget(bytes);
    // SAFETY: pointer and length are from a forgotten Vec with cap == len.
    let guard = unsafe { __rt::ResponseBytes::from_native(ptr, len) };
    let value = __rt::decode_value_bytes(guard.as_slice()).expect("decode");
    drop(guard);
    assert_eq!(value, Value::Str("ok".into()));
    assert_eq!(__rt::live_response_bytes(), 0);
}

#[test]
fn rv_sdk_a68c70de_malformed_import_response_fails_closed() {
    let error = __rt::decode_value_bytes(&[0x81, 0xa1]).expect_err("malformed");
    assert_eq!(error.kind(), ErrorKind::Codec);
    assert!(error.message().contains("decode host response"));
}

#[test]
fn rv_sdk_a68c70de_zero_len_is_absent_not_null() {
    assert_eq!(__rt::unpack_ptr_len(0), None);
    let packed = ((7i64) << 32) | 1;
    assert_eq!(__rt::unpack_ptr_len(packed), Some((7, 1)));
    assert_eq!(
        __rt::decode_value_bytes(&[0xc0]).expect("encoded null"),
        Value::Null
    );
}

#[test]
fn rv_sdk_b5acd67d_value_codec_roundtrips_nested_value() {
    let value = Value::List(vec![Value::Int(7), Value::Str("x".into())]);
    let bytes = __rt::encode_value(&value).expect("encode");
    assert_eq!(__rt::decode_value_bytes(&bytes).expect("decode"), value);
}

#[test]
fn rv_sdk_6d2497ca_malformed_http_envelope_and_response_are_codec() {
    let malformed = [
        Value::Int(1),
        Value::List(vec![Value::Int(1), Value::Null]),
        Value::List(vec![
            Value::Bool(true),
            Value::Map(vec![
                ("status".into(), Value::Int(200)),
                ("body".into(), Value::Str("bad".into())),
            ]),
        ]),
    ];
    for value in malformed {
        assert_eq!(
            crate::http::decode_fetch_envelope(&value)
                .unwrap_err()
                .kind(),
            ErrorKind::Codec
        );
    }
    let denial = crate::http::decode_fetch_envelope(&Value::List(vec![
        Value::Bool(false),
        Value::Str("denied".into()),
    ]))
    .unwrap_err();
    assert_eq!(denial.kind(), ErrorKind::User);
}
