//! RV-DB-C9444758 — CRLF/config-directive injection via guest-controlled
//! method and header names/values in curl config files.
//!
//! Every guest-controlled string that reaches the curl config file must be
//! rejected before any network/DNS work when it contains CR, LF, other C0
//! controls or DEL.

use crate::shamir_db::curl_gateway::{validate_curl_request, CurlNetGateway};
use shamir_engine::function::{HttpRequest, NetGateway};

const REVIEW_TEST_URL: &str = "https://non-routable-invalid-host-for-review-test.invalid/";

fn req_with_method(method: &str) -> HttpRequest {
    HttpRequest {
        method: method.to_string(),
        url: "https://example.com/".to_string(),
        headers: Vec::new(),
        body: Vec::new(),
    }
}

fn req_with_header(name: &str, value: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".to_string(),
        url: "https://example.com/".to_string(),
        headers: vec![(name.to_string(), value.to_string())],
        body: Vec::new(),
    }
}

#[test]
fn rv_db_c9444758_crlf_method_rejected() {
    let req = req_with_method("\nrequest = \"http://evil\"\n#");
    let err = validate_curl_request(&req).expect_err("LF in method must be rejected");
    assert!(err.contains("request method"), "got: {err}");
}

#[test]
fn rv_db_c9444758_cr_method_rejected() {
    let req = req_with_method("GE\rT");
    let err = validate_curl_request(&req).expect_err("CR in method must be rejected");
    assert!(err.contains("request method"), "got: {err}");
}

#[test]
fn rv_db_c9444758_crlf_header_value_rejected() {
    let req = req_with_header("X-Token", "\nrequest = \"http://evil\"\n#");
    let err = validate_curl_request(&req).expect_err("LF in header value must be rejected");
    assert!(err.contains("header value"), "got: {err}");
}

#[test]
fn rv_db_c9444758_crlf_header_name_rejected() {
    let req = req_with_header("X-Evil\nrequest = \"http://evil\"", "v");
    let err = validate_curl_request(&req).expect_err("LF in header name must be rejected");
    assert!(err.contains("header name"), "got: {err}");
}

#[test]
fn rv_db_c9444758_other_c0_and_del_rejected() {
    let req = req_with_header("X-A", "a\u{1}b");
    assert!(validate_curl_request(&req).is_err(), "C0 control rejected");
    let req = req_with_header("X-B", "a\u{7F}b");
    assert!(validate_curl_request(&req).is_err(), "DEL rejected");
}

#[test]
fn rv_db_c9444758_tab_in_header_value_allowed() {
    let req = req_with_header("X-Tab", "a\tb");
    assert!(
        validate_curl_request(&req).is_ok(),
        "horizontal tab in header value is permitted"
    );
}

// ── Fetch-level tests through the production entry point ────────────────
//
// These exercise `CurlNetGateway::new(vec![]).fetch(req)` (empty allowlist =
// deny everything), not just the validator helper. The empty allowlist makes
// `check_url_allowed_resolved` deny at its string-level check BEFORE any DNS
// resolution, so the denial message ("egress to {host} not allowed") is the
// discriminator for "validation passed and fetch proceeded past step 0".

#[tokio::test]
async fn rv_db_c9444758_fetch_crlf_header_value_rejected_before_dns() {
    let req = HttpRequest {
        method: "GET".to_string(),
        url: REVIEW_TEST_URL.to_string(),
        headers: vec![(
            "X-Token".to_string(),
            "\nrequest = \"http://evil\"\n#".to_string(),
        )],
        body: Vec::new(),
    };
    let err = CurlNetGateway::new(vec![])
        .fetch(req)
        .await
        .expect_err("LF in header value must be rejected before any DNS work")
        .to_string();
    assert!(
        err.contains("invalid control character") && err.contains("header value"),
        "expected step-0 validation error, got: {err}"
    );
}

#[tokio::test]
async fn rv_db_c9444758_fetch_crlf_method_rejected_before_dns() {
    let req = HttpRequest {
        method: "\nrequest = \"http://evil\"\n#".to_string(),
        url: REVIEW_TEST_URL.to_string(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    let err = CurlNetGateway::new(vec![])
        .fetch(req)
        .await
        .expect_err("LF in method must be rejected before any DNS work")
        .to_string();
    assert!(
        err.contains("invalid control character") && err.contains("request method"),
        "expected step-0 validation error, got: {err}"
    );
}

#[tokio::test]
async fn rv_db_c9444758_fetch_tab_header_passes_validation() {
    let req = HttpRequest {
        method: "GET".to_string(),
        url: REVIEW_TEST_URL.to_string(),
        headers: vec![("X-Tab".to_string(), "a\tb".to_string())],
        body: Vec::new(),
    };
    let err = CurlNetGateway::new(vec![])
        .fetch(req)
        .await
        .expect_err("empty allowlist must deny before any network I/O")
        .to_string();
    assert!(
        !err.contains("invalid control character"),
        "tab must pass step-0 validation; fetch should have proceeded to the \
         allowlist guard, got: {err}"
    );
}
