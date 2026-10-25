//! Regression tests for RV-WASM-HOST-4C69CFE0: duplicate HTTP response
//! headers must survive the host wire encoder (pair-list form, not map).

use crate::gateway::net_gateway::HttpResponse;
use crate::wasm::bindings::http::{decode_http_request, encode_http_response};
use shamir_types::types::value::QueryValue;

#[test]
fn rv_wasm_host_4c69cfe0_response_duplicate_headers_survive_encode() {
    let resp = HttpResponse {
        status: 200,
        headers: vec![
            ("Set-Cookie".to_string(), "a=1".to_string()),
            ("Set-Cookie".to_string(), "b=2".to_string()),
        ],
        body: Vec::new(),
    };

    let val = encode_http_response(resp);
    let map = match &val {
        QueryValue::Map(map) => map,
        other => panic!("expected Map, got {other:?}"),
    };
    let pairs = match map.get("headers") {
        Some(QueryValue::List(items)) => items,
        other => panic!("headers must be a List, got {other:?}"),
    };
    let cookies: Vec<&str> = pairs
        .iter()
        .filter_map(|item| match item {
            QueryValue::List(pair) if pair.len() == 2 => match (&pair[0], &pair[1]) {
                (QueryValue::Str(k), QueryValue::Str(v)) if k == "Set-Cookie" => Some(v.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(cookies, vec!["a=1", "b=2"]);
}

#[test]
fn rv_wasm_host_4c69cfe0_request_duplicate_headers_round_trip() {
    // Note: this test pins DECODER support for the pair-list form only. It
    // passes even with the response ENCODER fix reverted; the discriminating
    // test for the encoder is
    // `rv_wasm_host_4c69cfe0_response_duplicate_headers_survive_encode`.
    // Pair-list form (host-supported) with duplicate names.
    let wire = QueryValue::Map(
        [
            ("method".to_string(), QueryValue::Str("GET".to_string())),
            ("url".to_string(), QueryValue::Str("https://x/".to_string())),
            (
                "headers".to_string(),
                QueryValue::List(vec![
                    QueryValue::List(vec![
                        QueryValue::Str("Set-Cookie".to_string()),
                        QueryValue::Str("a=1".to_string()),
                    ]),
                    QueryValue::List(vec![
                        QueryValue::Str("Set-Cookie".to_string()),
                        QueryValue::Str("b=2".to_string()),
                    ]),
                ]),
            ),
            ("body".to_string(), QueryValue::Bin(Vec::new())),
        ]
        .into_iter()
        .collect(),
    );

    let req = decode_http_request(&wire).expect("decode");
    let cookies: Vec<&(String, String)> = req
        .headers
        .iter()
        .filter(|(k, _)| k == "Set-Cookie")
        .collect();
    assert_eq!(
        cookies,
        vec![
            &("Set-Cookie".to_string(), "a=1".to_string()),
            &("Set-Cookie".to_string(), "b=2".to_string()),
        ]
    );
}
