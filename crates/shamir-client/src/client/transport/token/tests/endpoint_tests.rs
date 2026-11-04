use std::net::SocketAddr;

use crate::client::transport::token::{TokenEndpoint, TokenTrust};
use crate::error::ClientError;

fn addr(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

fn https_ep(a: &str) -> TokenEndpoint {
    TokenEndpoint::https(addr(a), "db.example", TokenTrust::webpki_roots())
}

#[test]
fn https_accessors() {
    let ep = https_ep("192.0.2.1:8443");
    assert_eq!(ep.addr(), addr("192.0.2.1:8443"));
    assert_eq!(ep.server_name(), "db.example");
}

#[test]
fn host_header_omits_port_443() {
    let ep = https_ep("192.0.2.1:443");
    assert_eq!(ep.host_header(), "db.example");
}

#[test]
fn host_header_keeps_other_port() {
    let ep = https_ep("192.0.2.1:8443");
    assert_eq!(ep.host_header(), "db.example:8443");
}

#[test]
fn host_header_plain_ipv6_is_bracketed() {
    let ep = TokenEndpoint::plain_loopback(addr("[::1]:8080")).unwrap();
    assert_eq!(ep.host_header(), "[::1]:8080");
}

#[test]
fn plain_loopback_accepts_loopbacks() {
    for a in ["127.0.0.1:80", "[::1]:80", "[::ffff:127.0.0.1]:80"] {
        let ep = TokenEndpoint::plain_loopback(addr(a)).unwrap();
        assert_eq!(ep.addr(), addr(a));
    }
}

#[test]
fn plain_loopback_server_name_is_ip() {
    let ep = TokenEndpoint::plain_loopback(addr("127.0.0.1:80")).unwrap();
    assert_eq!(ep.server_name(), "127.0.0.1");
    assert_eq!(ep.host_header(), "127.0.0.1:80");
}

#[test]
fn plain_loopback_refuses_non_loopback() {
    for a in ["192.0.2.1:80", "0.0.0.0:80"] {
        let err = TokenEndpoint::plain_loopback(addr(a)).unwrap_err();
        assert!(matches!(err, ClientError::Handshake { .. }));
    }
}
