//! Regression tests for RV-WASM-HOST-4C69CFE0: duplicate HTTP headers
//! must survive the wire codec (pair-list form, not map).

use crate::api::http::HttpRequest;
use crate::Value;

#[test]
fn rv_wasm_host_4c69cfe0_request_duplicate_headers_survive_wire() {
    let req = HttpRequest::get("https://example.com/")
        .header("Set-Cookie", "a=1")
        .header("Set-Cookie", "b=2");

    let val = req.to_value();
    let entries = match &val {
        Value::Map(entries) => entries,
        other => panic!("expected Map, got {other:?}"),
    };
    let (_, headers) = entries
        .iter()
        .find(|(k, _)| k == "headers")
        .expect("headers field");

    let pairs = match headers {
        Value::List(items) => items,
        other => panic!("headers must be a List, got {other:?}"),
    };
    let cookies: Vec<&str> = pairs
        .iter()
        .filter_map(|item| match item {
            Value::List(pair) if pair.len() == 2 => match (&pair[0], &pair[1]) {
                (Value::Str(k), Value::Str(v)) if k == "Set-Cookie" => Some(v.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(cookies, vec!["a=1", "b=2"]);
}

#[test]
fn l3_rv_sdk_b5acd67d_http_response_rejects_malformed_envelopes() {
    let invalid = [
        Value::Null,
        Value::Map(vec![]),
        Value::Map(vec![("status".into(), Value::Int(700))]),
        Value::Map(vec![("status".into(), Value::Str("200".into()))]),
        Value::Map(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::List(vec![Value::Str("bad".into())]),
            ),
        ]),
        Value::Map(vec![
            ("status".into(), Value::Int(200)),
            (
                "headers".into(),
                Value::Map(vec![("x".into(), Value::Int(1))]),
            ),
        ]),
        Value::Map(vec![
            ("status".into(), Value::Int(200)),
            ("body".into(), Value::Str("not binary".into())),
        ]),
    ];
    for value in invalid {
        assert!(
            crate::api::http::HttpResponse::from_value(&value).is_err(),
            "{value:?}"
        );
    }
}

#[test]
fn l3_rv_sdk_b5acd67d_http_response_rejects_duplicate_top_level_fields() {
    for field in ["status", "headers", "body"] {
        let value = if field == "status" {
            Value::Map(vec![
                ("status".into(), Value::Int(200)),
                ("status".into(), Value::Int(201)),
            ])
        } else if field == "headers" {
            Value::Map(vec![
                ("status".into(), Value::Int(200)),
                ("headers".into(), Value::List(vec![])),
                ("body".into(), Value::Bin(vec![])),
                ("headers".into(), Value::List(vec![])),
            ])
        } else {
            Value::Map(vec![
                ("status".into(), Value::Int(200)),
                ("headers".into(), Value::List(vec![])),
                ("body".into(), Value::Bin(vec![])),
                ("body".into(), Value::Bin(vec![])),
            ])
        };
        assert!(
            crate::api::http::HttpResponse::from_value(&value).is_err(),
            "{field}"
        );
    }
}

#[test]
fn l3_rv_sdk_b5acd67d_http_response_decodes_exact_envelope() {
    let wire = Value::Map(vec![
        ("status".into(), Value::Int(201)),
        (
            "headers".into(),
            Value::List(vec![Value::List(vec![
                Value::Str("X-Test".into()),
                Value::Str("yes".into()),
            ])]),
        ),
        ("body".into(), Value::Bin(vec![0, 255, 1])),
    ]);
    let response = crate::api::http::HttpResponse::from_value(&wire).expect("valid envelope");
    assert_eq!(response.status(), 201);
    assert_eq!(
        response.headers(),
        &[("X-Test".to_string(), "yes".to_string())]
    );
    assert_eq!(response.body(), &[0, 255, 1]);
}
#[test]
fn rv_wasm_host_4c69cfe0_response_duplicate_headers_round_trip() {
    // Host-style wire value: pair-list headers with two Set-Cookie entries.
    let wire = Value::Map(vec![
        ("status".to_string(), Value::Int(200)),
        (
            "headers".to_string(),
            Value::List(vec![
                Value::List(vec![
                    Value::Str("Set-Cookie".to_string()),
                    Value::Str("a=1".to_string()),
                ]),
                Value::List(vec![
                    Value::Str("Set-Cookie".to_string()),
                    Value::Str("b=2".to_string()),
                ]),
            ]),
        ),
        ("body".to_string(), Value::Bin(Vec::new())),
    ]);

    let resp = crate::api::http::HttpResponse::from_value(&wire).expect("decode");
    let cookies: Vec<(String, String)> = resp
        .headers()
        .iter()
        .filter(|(k, _)| k == "Set-Cookie")
        .cloned()
        .collect();
    assert_eq!(
        cookies,
        vec![
            ("Set-Cookie".to_string(), "a=1".to_string()),
            ("Set-Cookie".to_string(), "b=2".to_string()),
        ]
    );
}
