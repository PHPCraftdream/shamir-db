//! HTTP and parameter contract regressions consolidated from former top-level module.

use crate::api::http::{HttpRequest, HttpResponse};
use crate::error::ErrorKind;
use crate::Value;

#[test]
fn rv_sdk_8bb269af_http_request_rejects_invalid_and_allows_path_at() {
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
    for invalid_url in [
        "https://example.test:invalid/path",
        "https:///missing-host",
        "https://example.test:",
        "https://[::1",
        "https://[not-ipv6]/",
        "https://example.test:65536/",
        "https://a..b/",
    ] {
        assert!(
            HttpRequest::get(invalid_url).try_to_value().is_err(),
            "invalid URL accepted: {invalid_url}"
        );
    }
    assert!(HttpRequest::get("https://[::1]:443/path/@x?q=@y")
        .try_to_value()
        .is_ok());
}

#[test]
fn rv_sdk_e6e616ed_http_response_rejects_lossy_fields() {
    let invalid = [
        Value::Map(vec![("status".into(), Value::Int(-1))]),
        Value::Map(vec![("status".into(), Value::Int(65_536))]),
        Value::Map(vec![
            ("status".into(), Value::Int(200)),
            ("headers".into(), Value::List(vec![Value::Int(0)])),
        ]),
        Value::Map(vec![
            ("status".into(), Value::Int(200)),
            ("body".into(), Value::Str("bad".into())),
        ]),
    ];
    for value in &invalid {
        assert!(HttpResponse::from_value(value).is_err());
    }
}

#[test]
fn rv_sdk_b5acd67d_parameter_and_response_mappings_reject_malformed_values() {
    assert!(crate::__rt::decode_params(&[0x81, 0xa1]).is_err());
    let malformed = Value::Map(vec![
        ("status".into(), Value::Int(200)),
        ("headers".into(), Value::List(vec![Value::Int(0)])),
    ]);
    assert_eq!(
        HttpResponse::from_value(&malformed).unwrap_err().kind(),
        ErrorKind::Codec
    );
}

#[test]
fn rv_sdk_b5acd67d_getters_and_table_mapping_are_preserved() {
    let params = crate::Params::from_map(vec![
        ("n".into(), Value::Int(9)),
        ("label".into(), Value::Str("x".into())),
    ]);
    assert_eq!(params.i64("n").unwrap(), 9);
    assert_eq!(params.str("label").unwrap(), "x");
    assert_eq!(params.get("missing").unwrap_err().kind(), ErrorKind::User);
    let mistyped = crate::Params::from_map(vec![("n".into(), Value::Str("wrong".into()))]);
    assert!(mistyped
        .i64("n")
        .unwrap_err()
        .message()
        .contains("expected integer"));
    assert_eq!(crate::Params::new().raw().len(), 0);
    assert_eq!(crate::Ctx::new().db().table("items").name(), "items");
}
