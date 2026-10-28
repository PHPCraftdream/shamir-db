//! Integrated SDK contract checks and public getter mappings.

use crate::api::http::{HttpRequest, HttpResponse};
use crate::error::ErrorKind;
use crate::Value;

#[test]
fn rv_sdk_8bb269af_http_request_rejects_invalid_tokens_controls_and_authority_userinfo() {
    assert!(HttpRequest::get("https://example.test")
        .method("GET\r\nX: y")
        .try_to_value()
        .is_err());
    assert!(HttpRequest::get("https://example.test")
        .header("X\nInjected", "v")
        .try_to_value()
        .is_err());
    assert!(HttpRequest::get("https://example.test")
        .header("x-ok", "v\r\nInjected: x")
        .try_to_value()
        .is_err());
    assert!(HttpRequest::get("https://example.test/path/@x?q=@y")
        .try_to_value()
        .is_ok());
    assert!(HttpRequest::get("https://user@example.test/")
        .try_to_value()
        .is_err());
}

#[test]
fn rv_sdk_e6e616ed_http_response_rejects_lossy_fields() {
    let invalid = [
        Value::Map(vec![("status".into(), Value::Int(-1))]),
        Value::Map(vec![("status".into(), Value::Int(65_536))]),
        Value::Map(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![Value::List(vec![Value::Str("ok".into())])]),
            ),
        ]),
        Value::Map(vec![
            ("status".into(), Value::Int(200)),
            ("body".into(), Value::Str("body".into())),
        ]),
    ];
    for value in &invalid {
        assert!(HttpResponse::from_value(value).is_err());
    }
}

#[test]
fn rv_sdk_23e9ee61_call_argument_category_is_invalid_argument() {
    let error = crate::Ctx::new().call("callee", Value::Int(1)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidArgument);
    assert!(error.message().contains("Value::Map"));
}

#[test]
fn rv_sdk_6d2497ca_error_categories_remain_distinct() {
    assert_eq!(crate::Error::user("bad input").kind(), ErrorKind::User);
    assert_eq!(crate::Error::codec("bad bytes").kind(), ErrorKind::Codec);
    assert_eq!(
        crate::Error::host("host unavailable").kind(),
        ErrorKind::Host
    );
}

#[test]
fn rv_sdk_b5acd67d_parameter_and_http_response_mappings_reject_malformed_fields() {
    let malformed = Value::Map(vec![
        ("status".into(), Value::Int(200)),
        ("headers".into(), Value::List(vec![Value::Int(0)])),
    ]);
    assert!(HttpResponse::from_value(&malformed).is_err());
    let malformed_body = Value::Map(vec![
        ("status".into(), Value::Int(200)),
        ("body".into(), Value::Int(1)),
    ]);
    assert!(HttpResponse::from_value(&malformed_body).is_err());
    assert!(crate::__rt::decode_params(&[0x81, 0xa1]).is_err());
}

#[test]
fn rv_sdk_b5acd67d_value_getters_preserve_mapping_contracts() {
    let params = crate::Params::from_map(vec![
        ("n".into(), Value::Int(9)),
        ("label".into(), Value::Str("x".into())),
    ]);
    assert_eq!(params.i64("n").unwrap(), 9);
    assert_eq!(params.str("label").unwrap(), "x");
    assert_eq!(params.get("missing").unwrap_err().kind(), ErrorKind::User);
    let mistyped = crate::Params::from_map(vec![("n".into(), Value::Str("not int".into()))]);
    assert!(mistyped
        .i64("n")
        .unwrap_err()
        .message()
        .contains("expected integer"));
    assert_eq!(crate::Params::new().raw().len(), 0);
    assert_eq!(crate::Ctx::new().db().table("items").name(), "items");
}

#[test]
fn rv_sdk_6d2497ca_malformed_host_response_is_codec_not_host() {
    let error = crate::__rt::decode_value_bytes(&[0x81, 0xa1]).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Codec);
}
