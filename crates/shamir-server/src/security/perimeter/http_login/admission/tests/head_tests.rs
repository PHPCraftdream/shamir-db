use axum::http::{HeaderMap, HeaderName, HeaderValue};
use std::net::{IpAddr, SocketAddr};

use shamir_transport_http::{LoginHead, PeerInfo};

use super::super::{LoginHeadPolicy, Source};

fn head(fields: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in fields {
        headers.append(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    headers
}
fn peer(addr: &str, forwarded: &[&str], tls: bool) -> PeerInfo {
    PeerInfo {
        addr: addr.parse::<SocketAddr>().unwrap(),
        tls,
        forwarded_for: forwarded.iter().map(|s| s.as_bytes().to_vec()).collect(),
    }
}

#[test]
fn host_and_origin_validation() {
    let policy = LoginHeadPolicy::new(
        1,
        vec!["example.test".into()],
        vec!["https://ui.test".into()],
        vec![],
        "127.0.0.1:443".parse().unwrap(),
    )
    .unwrap();
    for (fields, expected) in [
        (
            vec![("host", "example.test"), ("origin", "https://ui.test")],
            None,
        ),
        (vec![("host", "EXAMPLE.TEST:443")], None),
        (vec![("host", "example.test/path")], Some("invalid_request")),
        (
            vec![("host", "example.test"), ("host", "example.test")],
            Some("invalid_request"),
        ),
        (
            vec![("host", "example.test"), ("origin", "null")],
            Some("origin_forbidden"),
        ),
        (
            vec![("host", "example.test"), ("origin", "https://ui.test/path")],
            Some("origin_forbidden"),
        ),
        (
            vec![("host", "example.test"), ("origin", "HTTPS://ui.test")],
            Some("origin_forbidden"),
        ),
        (
            vec![("host", "example.test"), ("origin", "https://ui.test:443")],
            Some("origin_forbidden"),
        ),
        (
            vec![
                ("host", "example.test"),
                ("origin", "https://ui.test"),
                ("origin", "https://ui.test"),
            ],
            Some("origin_forbidden"),
        ),
        (vec![("host", "other.test")], Some("authority_forbidden")),
        (vec![("host", "bad..test")], Some("invalid_request")),
        (vec![("host", "-bad.test")], Some("invalid_request")),
        (vec![("host", "example.test:+443")], Some("invalid_request")),
        (vec![("host", "[::1]bad")], Some("invalid_request")),
    ] {
        let headers = head(&fields);
        let request = LoginHead::new(b"POST", b"/", &headers);
        let result = policy.resolve(&request, &peer("192.0.2.4:88", &[], true));
        assert_eq!(result.err().map(|e| e.code), expected);
    }
}

#[test]
fn proxy_chain_trust_walk_and_forwarded_duplicates() {
    let policy = LoginHeadPolicy::new(
        2,
        vec!["example.test".into()],
        vec!["https://ui.test".into()],
        vec!["10.0.0.0/8".into()],
        "127.0.0.1:80".parse().unwrap(),
    )
    .unwrap();
    let headers = head(&[("host", "example.test")]);
    let request = LoginHead::new(b"POST", b"/", &headers);
    let got = policy
        .resolve(
            &request,
            &peer(
                "10.2.0.1:80",
                &["malformed, 198.51.100.6", "10.1.0.1"],
                false,
            ),
        )
        .unwrap();
    assert_eq!(
        got,
        Source::from_ip("198.51.100.6".parse::<IpAddr>().unwrap())
    );
    for (proxy, chain, code) in [
        ("192.0.2.1:80", &["198.51.100.1"][..], "proxy_forbidden"),
        ("10.0.0.1:80", &[][..], "invalid_request"),
        ("10.0.0.1:80", &["10.0.0.2"][..], "invalid_request"),
        ("10.0.0.1:80", &["198.51.100.1, bad"][..], "invalid_request"),
    ] {
        assert_eq!(
            policy
                .resolve(&request, &peer(proxy, chain, false))
                .unwrap_err()
                .code,
            code
        );
    }
}

#[test]
fn mapped_ipv4_trusted_proxy_and_forwarded_source() {
    let policy = LoginHeadPolicy::new(
        2,
        vec!["example.test".into()],
        vec!["https://ui.test".into()],
        vec!["127.0.0.0/8".into()],
        "127.0.0.1:80".parse().unwrap(),
    )
    .unwrap();
    let headers = head(&[("host", "example.test")]);
    let request = LoginHead::new(b"POST", b"/", &headers);
    let resolved = policy
        .resolve(
            &request,
            &peer(
                "[::ffff:127.0.0.1]:80",
                &["malformed, ::ffff:198.51.100.6"],
                false,
            ),
        )
        .unwrap();
    assert_eq!(
        resolved,
        Source::from_ip("198.51.100.6".parse::<IpAddr>().unwrap())
    );

    let broad_mapped = LoginHeadPolicy::new(
        2,
        vec!["example.test".into()],
        vec!["https://ui.test".into()],
        vec!["::ffff:127.0.0.0/104".into()],
        "127.0.0.1:80".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(
        broad_mapped
            .resolve(&request, &peer("192.0.2.1:80", &["198.51.100.6"], false))
            .unwrap_err()
            .code,
        "proxy_forbidden"
    );
}

#[test]
fn mapped_loopback_and_bound_port_defaults() {
    let policy =
        LoginHeadPolicy::new(0, vec![], vec![], vec![], "127.0.0.1:7340".parse().unwrap()).unwrap();
    let headers = head(&[("host", "localhost:7340")]);
    let request = LoginHead::new(b"POST", b"/", &headers);
    let mapped = peer("[::ffff:127.0.0.1]:80", &[], false);
    assert_eq!(
        policy.resolve(&request, &mapped).unwrap(),
        Source::from_ip("127.0.0.1".parse().unwrap())
    );
}
