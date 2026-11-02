use std::net::SocketAddr;

use super::super::browser_policy::{canonical_authority, host_allowed, valid_origin};
use super::super::first_frame::FIRST_FRAME_MAX;
use super::super::upgrade::validate_head;
use shamir_transport_http::server::routes::ws::{UpgradeHead, WsRoute};
use tokio_tungstenite::tungstenite::http::header::{CONNECTION, ORIGIN};
use tokio_tungstenite::tungstenite::http::{HeaderMap, HeaderValue, Method};

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
fn head_rejects_duplicate_origin_and_requires_host_and_origin_for_browser() {
    let policy = vec!["https://app.example".to_owned()];
    let peer: SocketAddr = "127.0.0.1:9".parse().expect("address");
    let mut headers = good_headers();
    headers.append(ORIGIN, HeaderValue::from_static("https://app.example"));
    assert_eq!(
        validate_head(
            WsRoute::Native,
            UpgradeHead {
                path: "/shamir/v1/ws",
                method: &Method::GET,
                headers: &headers
            },
            false,
            false,
            &policy,
            &[]
        )
        .err()
        .expect("duplicate origin rejection")
        .status,
        tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN
    );
    let headers = good_headers();
    assert!(validate_head(
        WsRoute::Browser,
        UpgradeHead {
            path: "/shamir/v1/ws/browser",
            method: &Method::GET,
            headers: &headers
        },
        false,
        false,
        &policy,
        &["localhost".into()]
    )
    .is_err());
}

#[test]
fn first_frame_limit_is_fixed_before_payload_allocation() {
    assert_eq!(FIRST_FRAME_MAX, 256);
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
