use std::time::Duration;

use hyper::header::HeaderValue;
use tokio::net::TcpListener;

use crate::client::transport::token::{
    build_connector, parse_server_name, TokenEndpoint, TokenTransport, TokenTrust,
};
use crate::error::ClientError;

#[test]
fn webpki_roots_connector_builds() {
    assert!(build_connector(&TokenTrust::webpki_roots()).is_ok());
}

#[test]
fn pem_empty_is_rejected() {
    let err = TokenTrust::pem(b"").unwrap_err();
    assert!(matches!(err, ClientError::Tls { .. }));
}

#[test]
fn pem_garbage_is_rejected() {
    let err = TokenTrust::pem(b"not a certificate").unwrap_err();
    assert!(matches!(err, ClientError::Tls { .. }));
}

#[test]
fn garbage_der_fails_to_build() {
    let trust = TokenTrust::der(vec![1u8, 2, 3]);
    assert!(build_connector(&trust).is_err());
}

#[test]
fn empty_custom_roots_fail_to_build() {
    assert!(build_connector(&TokenTrust::CustomRoots(Vec::new())).is_err());
}

#[test]
fn server_name_accepts_dns_and_ip() {
    assert!(parse_server_name("localhost").is_ok());
    assert!(parse_server_name("127.0.0.1").is_ok());
}

#[test]
fn server_name_rejects_invalid() {
    assert!(parse_server_name("bad name!").is_err());
}

/// The TCP dial deadline of `connect_once_with_tcp_deadline`: an
/// already-elapsed deadline fires the typed `ConnectTimeout` BEFORE any
/// dial I/O (never as the refused socket's `Io` error), while a future
/// deadline against a live listener yields a deterministic `Ok`.
#[tokio::test]
async fn tcp_dial_deadline_bounds_only_the_dial() {
    // Kept bound for the whole test so the positive case connects
    // deterministically instead of racing a dropped port.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let endpoint = TokenEndpoint::plain_loopback(addr).expect("loopback endpoint");
    let dial = |deadline: Option<tokio::time::Instant>| {
        TokenTransport::connect_once_with_tcp_deadline(
            &endpoint,
            HeaderValue::from_static("Bearer x"),
            None,
            None,
            deadline,
        )
    };
    let error = dial(Some(tokio::time::Instant::now()))
        .await
        .err()
        .expect("expired deadline");
    assert!(
        matches!(error, ClientError::ConnectTimeout(_)),
        "expired deadline must be a typed ConnectTimeout, got {error:?}"
    );
    dial(Some(tokio::time::Instant::now() + Duration::from_secs(10)))
        .await
        .expect("live listener accepts a future deadline");
}
