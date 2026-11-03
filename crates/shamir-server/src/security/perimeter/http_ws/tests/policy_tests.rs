use super::super::browser::first_frame::FIRST_FRAME_MAX;
use super::super::browser::policy::{canonical_authority, host_allowed, valid_origin};
use super::super::upgrade::{response_headers, validate_head};
use shamir_transport_http::server::routes::ws::{UpgradeHead, WsRoute};
use tokio_tungstenite::tungstenite::http::header::{CONNECTION, HOST, ORIGIN};
use tokio_tungstenite::tungstenite::http::{HeaderMap, HeaderValue, Method, StatusCode};

#[test]
fn canonical_authority_normalizes_valid_host_forms_and_rejects_injection() {
    assert_eq!(
        canonical_authority("LOCAL.example:80", false).as_deref(),
        Some("local.example")
    );
    assert_eq!(
        canonical_authority("[2001:0DB8::1]:443", true).as_deref(),
        Some("[2001:db8::1]")
    );
    for value in [
        "user@host",
        "host/path",
        "host:65536",
        "::1",
        "bad host",
        "host%00",
    ] {
        assert!(canonical_authority(value, false).is_none(), "{value}");
    }
    assert_eq!(
        valid_origin(b"https://LOCAL.example:443").as_deref(),
        Some("https://local.example")
    );
    assert!(valid_origin(b"null").is_none());
    assert!(host_allowed(
        b"LOCAL.example:80",
        &["local.example".into()],
        false
    ));
}

#[test]
fn browser_origin_validation_precedes_host_validation() {
    let origins = ["https://app.example".to_owned()];
    let hosts = ["localhost".to_owned()];

    for value in [None, Some("null"), Some("not an origin")] {
        let mut headers = good_headers();
        headers.insert(HOST, HeaderValue::from_static("localhost"));
        if let Some(value) = value {
            headers.insert(ORIGIN, HeaderValue::from_str(value).expect("header"));
        }
        assert_status(&headers, &origins, &hosts, StatusCode::BAD_REQUEST);
    }

    let mut malformed_headers = good_headers();
    malformed_headers.insert(HOST, HeaderValue::from_static("localhost"));
    malformed_headers.insert(ORIGIN, HeaderValue::from_static("not-an-origin"));
    assert_status(
        &malformed_headers,
        &origins,
        &hosts,
        StatusCode::BAD_REQUEST,
    );

    let mut duplicate_origin = good_browser_headers();
    duplicate_origin.append(ORIGIN, HeaderValue::from_static("https://app.example"));
    duplicate_origin.append(ORIGIN, HeaderValue::from_static("https://app.example"));
    assert_status(&duplicate_origin, &origins, &hosts, StatusCode::BAD_REQUEST);

    let mut unlisted_origin_and_duplicate_host = good_browser_headers();
    unlisted_origin_and_duplicate_host
        .insert(ORIGIN, HeaderValue::from_static("https://evil.example"));
    unlisted_origin_and_duplicate_host.append(HOST, HeaderValue::from_static("localhost"));
    assert_status(
        &unlisted_origin_and_duplicate_host,
        &origins,
        &hosts,
        StatusCode::FORBIDDEN,
    );
    let mut unlisted_origin = good_browser_headers();
    unlisted_origin.insert(ORIGIN, HeaderValue::from_static("https://evil.example"));
    assert_status(&unlisted_origin, &origins, &hosts, StatusCode::FORBIDDEN);
}

#[test]
fn browser_host_validation_and_normalization() {
    let origins = ["https://app.example".to_owned()];
    let hosts = ["localhost".to_owned()];
    for value in [None, Some("bad host"), Some("evil.example")] {
        let mut headers = good_browser_headers();
        if let Some(value) = value {
            headers.insert(HOST, HeaderValue::from_str(value).expect("header"));
        } else {
            headers.remove(HOST);
        }
        assert_status(&headers, &origins, &hosts, StatusCode::BAD_REQUEST);
    }

    let mut duplicate_host = good_browser_headers();
    duplicate_host.append(HOST, HeaderValue::from_static("localhost"));
    assert_status(&duplicate_host, &origins, &hosts, StatusCode::BAD_REQUEST);

    let mut normalized = good_browser_headers();
    normalized.insert(HOST, HeaderValue::from_static("LOCALHOST:80"));
    assert_browser_success(&normalized, &origins, &hosts);
}

#[test]
fn native_origin_requires_an_exact_nonempty_allowlist() {
    let headers = native_headers(Some("https://app.example"));
    assert_native_status(&headers, &[], StatusCode::FORBIDDEN);
    assert_native_status(
        &native_headers(Some("not-an-origin")),
        &["https://app.example".to_owned()],
        StatusCode::FORBIDDEN,
    );
    assert_native_success(&headers, &["https://app.example".to_owned()]);
    assert_native_status(
        &headers,
        &["https://other.example".to_owned()],
        StatusCode::FORBIDDEN,
    );
}

#[test]
fn response_headers_never_enable_extensions_and_select_protocol() {
    let checked = validate_head(
        WsRoute::Browser,
        browser_head(&good_browser_headers()),
        false,
        &["https://app.example".to_owned()],
        &["localhost".to_owned()],
    )
    .unwrap_or_else(|_| panic!("valid browser head"));
    let headers = response_headers(&checked.state);
    assert!(headers
        .iter()
        .any(|(name, value)| { name == "sec-websocket-protocol" && value == "shamir-v1" }));
    assert!(headers
        .iter()
        .all(|(name, _)| name != "sec-websocket-extensions"));
}

#[test]
fn first_frame_limit_is_fixed_before_payload_allocation() {
    assert_eq!(FIRST_FRAME_MAX, 256);
}

fn assert_status(headers: &HeaderMap, origins: &[String], hosts: &[String], expected: StatusCode) {
    let result = validate_head(
        WsRoute::Browser,
        browser_head(headers),
        false,
        origins,
        hosts,
    );
    assert_eq!(result.err().expect("rejected headers").status, expected);
}

fn assert_native_status(headers: &HeaderMap, origins: &[String], expected: StatusCode) {
    let result = validate_head(
        WsRoute::Native,
        UpgradeHead {
            path: "/shamir/v1/ws",
            method: &Method::GET,
            headers,
        },
        false,
        origins,
        &[],
    );
    assert_eq!(
        result.err().expect("rejected native headers").status,
        expected
    );
}

fn assert_browser_success(headers: &HeaderMap, origins: &[String], hosts: &[String]) {
    validate_head(
        WsRoute::Browser,
        browser_head(headers),
        false,
        origins,
        hosts,
    )
    .unwrap_or_else(|_| panic!("normalized allowed Host and Origin"));
}

fn assert_native_success(headers: &HeaderMap, origins: &[String]) {
    validate_head(
        WsRoute::Native,
        UpgradeHead {
            path: "/shamir/v1/ws",
            method: &Method::GET,
            headers,
        },
        false,
        origins,
        &[],
    )
    .unwrap_or_else(|_| panic!("matching native Origin"));
}

fn browser_head(headers: &HeaderMap) -> UpgradeHead<'_> {
    UpgradeHead {
        path: "/shamir/v1/ws/browser",
        method: &Method::GET,
        headers,
    }
}

fn good_browser_headers() -> HeaderMap {
    let mut headers = good_headers();
    headers.insert(HOST, HeaderValue::from_static("localhost"));
    headers.insert(ORIGIN, HeaderValue::from_static("https://app.example"));
    headers
}

fn native_headers(origin: Option<&'static str>) -> HeaderMap {
    let mut headers = good_headers();
    if let Some(origin) = origin {
        headers.insert(ORIGIN, HeaderValue::from_static(origin));
    }
    headers
}

fn good_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("upgrade", HeaderValue::from_static("websocket"));
    headers.insert(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
    headers.insert("sec-websocket-version", HeaderValue::from_static("13"));
    headers.insert(
        "sec-websocket-key",
        HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
    );
    headers.insert(
        "sec-websocket-protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    headers
}
