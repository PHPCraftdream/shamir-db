use std::net::SocketAddr;

use crate::listener::{validate_addr, WsBindError, WsListenerProfile};

fn sa(ip: &str, port: u16) -> SocketAddr {
    if ip.contains(':') {
        format!("[{}]:{}", ip, port).parse().unwrap()
    } else {
        format!("{}:{}", ip, port).parse().unwrap()
    }
}

#[test]
fn wss_allows_any_address() {
    assert!(WsListenerProfile::Wss.allows(&sa("8.8.8.8", 7332)));
    assert!(WsListenerProfile::Wss.allows(&sa("0.0.0.0", 7332)));
    assert!(WsListenerProfile::Wss.allows(&sa("::", 7332)));
}

#[test]
fn wss_browser_allows_any_address() {
    assert!(WsListenerProfile::WssBrowser.allows(&sa("8.8.8.8", 7333)));
    assert!(WsListenerProfile::WssBrowser.allows(&sa("0.0.0.0", 7333)));
}

#[test]
fn plain_loopback_allows_127_0_0_1() {
    assert!(WsListenerProfile::PlainWsLoopback.allows(&sa("127.0.0.1", 7334)));
    assert!(WsListenerProfile::PlainWsLoopback.allows(&sa("127.255.255.254", 7334)));
    assert!(WsListenerProfile::PlainWsLoopback.allows(&sa("::1", 7334)));
}

#[test]
fn plain_loopback_refuses_lan() {
    assert!(!WsListenerProfile::PlainWsLoopback.allows(&sa("192.168.1.5", 7334)));
    assert!(!WsListenerProfile::PlainWsLoopback.allows(&sa("8.8.8.8", 7334)));
}

#[test]
fn plain_loopback_refuses_unspecified() {
    // 0.0.0.0 / :: bind to all interfaces — explicit guard.
    assert!(!WsListenerProfile::PlainWsLoopback.allows(&sa("0.0.0.0", 7334)));
    assert!(!WsListenerProfile::PlainWsLoopback.allows(&sa("::", 7334)));
}

#[test]
fn validate_addr_rejects_plain_ws_on_lan() {
    let r = validate_addr(sa("192.168.1.5", 7334), WsListenerProfile::PlainWsLoopback);
    assert!(matches!(r, Err(WsBindError::PlainOnNonLoopback(_))));
}

#[test]
fn validate_addr_passes_for_wss_on_lan() {
    assert!(validate_addr(sa("192.168.1.5", 7332), WsListenerProfile::Wss).is_ok());
}

#[tokio::test]
async fn bind_validated_succeeds_for_plain_loopback() {
    let l = crate::listener::bind_validated(sa("127.0.0.1", 0), WsListenerProfile::PlainWsLoopback)
        .await
        .unwrap();
    assert!(l.local_addr().unwrap().ip().is_loopback());
}

#[tokio::test]
async fn bind_validated_refuses_plain_on_unspecified() {
    let r =
        crate::listener::bind_validated(sa("0.0.0.0", 0), WsListenerProfile::PlainWsLoopback).await;
    assert!(matches!(r, Err(WsBindError::PlainOnNonLoopback(_))));
}
