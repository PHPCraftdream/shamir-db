//! Tests for [`decode_http_request`] fail-closed wire-shape validation
//! (RV-WASM-HOST-D0B90CF6).
use crate::gateway::net_gateway::HttpRequest;
use crate::wasm::bindings::http::decode_http_request;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

fn req_map(entries: Vec<(&str, QueryValue)>) -> QueryValue {
    let mut map = new_map_wc(entries.len());
    for (k, v) in entries {
        map.insert(k.to_string(), v);
    }
    QueryValue::Map(map)
}

fn full_valid_entries() -> Vec<(&'static str, QueryValue)> {
    vec![
        ("method", QueryValue::Str("POST".to_string())),
        (
            "url",
            QueryValue::Str("https://api.example.com/v1".to_string()),
        ),
        (
            "headers",
            QueryValue::List(vec![QueryValue::List(vec![
                QueryValue::Str("X-Test".to_string()),
                QueryValue::Str("yes".to_string()),
            ])]),
        ),
        ("body", QueryValue::Bin(vec![1, 2, 3])),
    ]
}

#[test]
fn rv_wasm_host_d0b90cf6_missing_method_rejected() {
    let entries: Vec<(&str, QueryValue)> = full_valid_entries()
        .into_iter()
        .filter(|(k, _)| *k != "method")
        .collect();
    let err = decode_http_request(&req_map(entries)).unwrap_err();
    assert!(err.contains("missing method"), "got: {err}");
}

#[test]
fn rv_wasm_host_d0b90cf6_empty_method_rejected() {
    let mut entries = full_valid_entries();
    entries[0] = ("method", QueryValue::Str(String::new()));
    let err = decode_http_request(&req_map(entries)).unwrap_err();
    assert!(err.contains("method"), "got: {err}");
}

#[test]
fn rv_wasm_host_d0b90cf6_missing_url_rejected() {
    let entries: Vec<(&str, QueryValue)> = full_valid_entries()
        .into_iter()
        .filter(|(k, _)| *k != "url")
        .collect();
    let err = decode_http_request(&req_map(entries)).unwrap_err();
    assert!(err.contains("missing url"), "got: {err}");
}

#[test]
fn rv_wasm_host_d0b90cf6_empty_url_rejected() {
    let mut entries = full_valid_entries();
    entries[1] = ("url", QueryValue::Str(String::new()));
    let err = decode_http_request(&req_map(entries)).unwrap_err();
    assert!(err.contains("url"), "got: {err}");
}

#[test]
fn rv_wasm_host_d0b90cf6_malformed_headers_rejected() {
    let mut entries = full_valid_entries();
    entries[2] = ("headers", QueryValue::Str("X-Test: yes".to_string()));
    let err = decode_http_request(&req_map(entries)).unwrap_err();
    assert!(err.contains("headers"), "got: {err}");
}

#[test]
fn rv_wasm_host_d0b90cf6_absent_headers_ok_empty() {
    let entries: Vec<(&str, QueryValue)> = full_valid_entries()
        .into_iter()
        .filter(|(k, _)| *k != "headers")
        .collect();
    let req = decode_http_request(&req_map(entries)).expect("absent headers must be allowed");
    assert!(req.headers.is_empty());
}

#[test]
fn rv_wasm_host_d0b90cf6_malformed_body_rejected() {
    let mut entries = full_valid_entries();
    entries[3] = ("body", QueryValue::Str("not binary".to_string()));
    let err = decode_http_request(&req_map(entries)).unwrap_err();
    assert_eq!(err, "http_fetch: body must be binary");

    let mut entries = full_valid_entries();
    entries[3] = ("body", QueryValue::Int(7));
    let err = decode_http_request(&req_map(entries)).unwrap_err();
    assert_eq!(err, "http_fetch: body must be binary");
}

#[test]
fn rv_wasm_host_d0b90cf6_absent_body_ok_empty() {
    let entries: Vec<(&str, QueryValue)> = full_valid_entries()
        .into_iter()
        .filter(|(k, _)| *k != "body")
        .collect();
    let req = decode_http_request(&req_map(entries)).expect("absent body must be allowed");
    assert!(req.body.is_empty());
}

#[test]
fn rv_wasm_host_d0b90cf6_happy_path_decodes_request() {
    let req: HttpRequest =
        decode_http_request(&req_map(full_valid_entries())).expect("valid request must decode");
    assert_eq!(req.method, "POST");
    assert_eq!(req.url, "https://api.example.com/v1");
    assert_eq!(req.headers, vec![("X-Test".to_string(), "yes".to_string())]);
    assert_eq!(req.body, vec![1, 2, 3]);
}
