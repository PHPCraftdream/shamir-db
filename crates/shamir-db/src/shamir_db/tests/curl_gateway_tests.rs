use crate::shamir_db::curl_gateway::{build_resolve_lines, escape_curl_value};
use shamir_engine::function::ResolvedPin;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[test]
fn escape_curl_value_backslash() {
    assert_eq!(escape_curl_value(r"a\b"), r"a\\b");
}

#[test]
fn escape_curl_value_quotes() {
    assert_eq!(escape_curl_value(r#"a"b"#), r#"a\"b"#);
}

#[test]
fn escape_curl_value_plain() {
    assert_eq!(escape_curl_value("hello"), "hello");
}

// ── Finding 2c: DNS-rebind pin wired into curl.cfg ───────────────────────

#[test]
fn resolve_line_pins_single_validated_ip() {
    // The curl config must carry `resolve = "host:port:ip"` with EXACTLY the
    // IP the SSRF guard validated, so curl connects to that address instead of
    // re-resolving the hostname (DNS-rebind window closed).
    let pin = ResolvedPin {
        host: "api.example.com".to_string(),
        port: 443,
        pinned_ips: vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
    };
    let lines = build_resolve_lines(&pin);
    assert_eq!(lines, "resolve = \"api.example.com:443:93.184.216.34\"\n");
}

#[test]
fn resolve_line_pins_all_validated_ips() {
    // When DNS returned several addresses (e.g. A + AAAA), pin ALL of them so
    // curl can only ever connect to a guard-validated address.
    let pin = ResolvedPin {
        host: "example.com".to_string(),
        port: 80,
        pinned_ips: vec![
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5)),
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
        ],
    };
    let lines = build_resolve_lines(&pin);
    assert_eq!(
        lines,
        "resolve = \"example.com:80:203.0.113.5\"\n\
         resolve = \"example.com:80:2001:db8::1\"\n"
    );
}

#[test]
fn resolve_line_empty_for_exact_allowlist_path() {
    // Exact-allowlist-match path performs no DNS resolution → empty pin set →
    // no `resolve` line is emitted (curl resolves the operator-allowed host).
    let pin = ResolvedPin {
        host: "127.0.0.1".to_string(),
        port: 8080,
        pinned_ips: vec![],
    };
    assert_eq!(build_resolve_lines(&pin), "");
}
