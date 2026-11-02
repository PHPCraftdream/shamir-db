use std::net::SocketAddr;

use shamir_transport_http::PeerInfo;

use super::super::super::proxy::{parse_cidr, resolve_peer};

fn peer(addr: &str, forwarded_for: &[&[u8]]) -> PeerInfo {
    PeerInfo {
        addr: addr.parse().expect("valid peer address"),
        tls: false,
        forwarded_for: forwarded_for.iter().map(|value| value.to_vec()).collect(),
    }
}

fn resolve(proxies: &[&str], peer: PeerInfo) -> SocketAddr {
    let trusted = proxies
        .iter()
        .map(|value| parse_cidr(value).unwrap())
        .collect::<Vec<_>>();
    resolve_peer(&peer, &trusted)
}

#[test]
fn trusted_chain_walks_right_to_left_across_headers() {
    let result = resolve(
        &["10.0.0.0/8"],
        peer("10.0.0.9:443", &[b"198.51.100.8, 10.0.0.2", b"10.0.0.3"]),
    );
    assert_eq!(result, "198.51.100.8:443".parse().unwrap());
}

#[test]
fn untrusted_peer_ignores_forwarded_headers() {
    let result = resolve(&["10.0.0.0/8"], peer("192.0.2.1:55", &[b"198.51.100.7"]));
    assert_eq!(result, "192.0.2.1:55".parse().unwrap());
}

#[test]
fn malformed_chain_falls_back_to_tcp_peer() {
    let result = resolve(
        &["10.0.0.0/8"],
        peer("10.0.0.9:443", &[b"198.51.100.2,not-an-ip"]),
    );
    assert_eq!(result, "10.0.0.9:443".parse().unwrap());
}

#[test]
fn all_trusted_chain_keeps_tcp_peer_and_cidr_boundaries_match() {
    assert_eq!(
        resolve(&["10.0.0.0/8"], peer("10.0.0.9:443", &[b"10.1.0.1"])),
        "10.0.0.9:443".parse().unwrap()
    );
    let ipv4 = parse_cidr("192.0.2.0/24").unwrap();
    assert!(ipv4.contains("192.0.2.255".parse().unwrap()));
    assert!(!ipv4.contains("192.0.3.0".parse().unwrap()));
    assert!(ipv4.contains("::ffff:192.0.2.255".parse().unwrap()));
    assert!(!ipv4.contains("::ffff:192.0.3.0".parse().unwrap()));
    let ipv6 = parse_cidr("2001:db8::/32").unwrap();
    assert!(ipv6.contains("2001:db8:ffff::1".parse().unwrap()));
    assert!(!ipv6.contains("2001:db9::1".parse().unwrap()));
    let mapped_v6_cidr = parse_cidr("::ffff:192.0.2.0/120").unwrap();
    assert!(mapped_v6_cidr.contains("::ffff:192.0.2.255".parse().unwrap()));
    assert!(!mapped_v6_cidr.contains("::ffff:192.0.3.0".parse().unwrap()));
    assert!(mapped_v6_cidr.contains("192.0.2.255".parse().unwrap()));
    assert!(!ipv6.contains("::ffff:192.0.2.9".parse().unwrap()));
    assert_eq!(
        resolve(
            &["::ffff:192.0.2.0/120"],
            peer("[::ffff:192.0.2.9]:443", &[b"[::ffff:198.51.100.7]:8443"]),
        ),
        "198.51.100.7:443".parse().unwrap()
    );
    assert!(parse_cidr("0.0.0.0/0")
        .unwrap()
        .contains("203.0.113.1".parse().unwrap()));
    assert!(parse_cidr("::/0")
        .unwrap()
        .contains("2001:db8::1".parse().unwrap()));
}

#[test]
fn headers_over_limit_fall_back_to_tcp_peer() {
    let oversized = "198.51.100.1, ".repeat(700);
    let oversized = oversized.trim_end_matches(", ");
    assert!(oversized.len() > 8192);
    let result = resolve(
        &["10.0.0.0/8"],
        peer("10.0.0.9:443", &[oversized.as_bytes()]),
    );
    assert_eq!(result, "10.0.0.9:443".parse().unwrap());
}

#[test]
fn mapped_peers_and_forwarded_addresses_resolve_to_canonical_ipv4() {
    let trusted = resolve(
        &["192.0.2.0/24"],
        peer("[::ffff:192.0.2.9]:443", &[b"::ffff:198.51.100.7"]),
    );
    assert_eq!(trusted, "198.51.100.7:443".parse().unwrap());

    let mapped_trusted_forwarded = resolve(
        &["192.0.2.0/24"],
        peer(
            "[::ffff:192.0.2.9]:443",
            &[b"::ffff:198.51.100.7, ::ffff:192.0.2.12"],
        ),
    );
    assert_eq!(
        mapped_trusted_forwarded,
        "198.51.100.7:443".parse().unwrap()
    );

    let untrusted_forwarded = resolve(
        &["10.0.0.0/8"],
        peer("[::ffff:192.0.2.9]:443", &[b"198.51.100.7"]),
    );
    assert_eq!(untrusted_forwarded, "192.0.2.9:443".parse().unwrap());
}

#[test]
fn direct_untrusted_mapped_peer_falls_back_to_canonical_ipv4() {
    assert_eq!(
        resolve(&["10.0.0.0/8"], peer("[::ffff:192.0.2.9]:443", &[b"bad"])),
        "192.0.2.9:443".parse().unwrap()
    );
}

#[test]
fn valid_forwarded_header_under_limit_is_parsed() {
    let result = resolve(&["10.0.0.0/8"], peer("10.0.0.9:443", &[b"198.51.100.1"]));
    assert_eq!(result, "198.51.100.1:443".parse().unwrap());
}
