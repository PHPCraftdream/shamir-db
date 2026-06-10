use std::net::SocketAddr;

use crate::listener::{validate_addr, ListenerBindError, ListenerProfile};

fn sa(ip: &str, port: u16) -> SocketAddr {
    // IPv6 needs square-bracket form for socket-addr parsing.
    if ip.contains(':') {
        format!("[{}]:{}", ip, port).parse().unwrap()
    } else {
        format!("{}:{}", ip, port).parse().unwrap()
    }
}

#[test]
fn plain_allows_127_0_0_1() {
    let addr = sa("127.0.0.1", 7331);
    assert!(ListenerProfile::Plain.allows(&addr));
}

#[test]
fn plain_allows_127_anything() {
    // 127.0.0.0/8 = all 127.x.x.x addresses.
    let addr = sa("127.255.255.254", 7331);
    assert!(ListenerProfile::Plain.allows(&addr));
}

#[test]
fn plain_allows_ipv6_localhost() {
    let addr = sa("::1", 7331);
    assert!(ListenerProfile::Plain.allows(&addr));
}

#[test]
fn plain_refuses_lan_address() {
    let addr = sa("192.168.1.5", 7331);
    assert!(!ListenerProfile::Plain.allows(&addr));
}

#[test]
fn plain_refuses_internet_address() {
    let addr = sa("8.8.8.8", 7331);
    assert!(!ListenerProfile::Plain.allows(&addr));
}

#[test]
fn plain_refuses_unspecified_zero_addr() {
    let addr = sa("0.0.0.0", 7331);
    assert!(!ListenerProfile::Plain.allows(&addr));
}

#[test]
fn plain_refuses_ipv6_unspecified() {
    let addr = sa("::", 7331);
    assert!(!ListenerProfile::Plain.allows(&addr));
}

#[test]
fn tls_profiles_allow_any_address() {
    let public = sa("8.8.8.8", 7331);
    let unspec = sa("0.0.0.0", 7331);
    assert!(ListenerProfile::TlsExporter.allows(&public));
    assert!(ListenerProfile::TlsExporter.allows(&unspec));
    assert!(ListenerProfile::TlsNoExport.allows(&public));
    assert!(ListenerProfile::TlsNoExport.allows(&unspec));
}

#[test]
fn validate_addr_returns_error_for_plain_on_lan() {
    let addr = sa("192.168.1.5", 7331);
    let result = validate_addr(addr, ListenerProfile::Plain);
    assert!(matches!(
        result,
        Err(ListenerBindError::PlainOnNonLoopback(_))
    ));
}

#[test]
fn validate_addr_passes_for_tls_on_lan() {
    let addr = sa("192.168.1.5", 7331);
    assert!(validate_addr(addr, ListenerProfile::TlsExporter).is_ok());
}

#[tokio::test]
async fn bind_validated_succeeds_for_plain_loopback() {
    let addr = sa("127.0.0.1", 0); // port 0 = OS-assigned
    let l = crate::listener::bind_validated(addr, ListenerProfile::Plain)
        .await
        .unwrap();
    let bound = l.local_addr().unwrap();
    assert!(bound.ip().is_loopback());
}

#[tokio::test]
async fn bind_validated_refuses_plain_on_unspecified() {
    let addr = sa("0.0.0.0", 0);
    let r = crate::listener::bind_validated(addr, ListenerProfile::Plain).await;
    assert!(matches!(r, Err(ListenerBindError::PlainOnNonLoopback(_))));
}

#[tokio::test]
async fn bind_validated_succeeds_for_tls_on_unspecified() {
    let addr = sa("127.0.0.1", 0); // use loopback to avoid firewall prompts in CI
    let _l = crate::listener::bind_validated(addr, ListenerProfile::TlsExporter)
        .await
        .unwrap();
}
