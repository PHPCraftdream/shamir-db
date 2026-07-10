use crate::net_gateway::{
    canonicalize_ip, check_host_allowed, check_url_allowed, check_url_allowed_resolved, parse_url,
};
use std::net::{IpAddr, Ipv4Addr};

#[test]
fn allowlist_exact_match() {
    let list = vec!["api.example.com".to_string()];
    assert!(check_host_allowed("api.example.com", &list).is_ok());
    assert!(check_host_allowed("other.example.com", &list).is_err());
}

#[test]
fn allowlist_wildcard() {
    let list = vec!["*.example.com".to_string()];
    assert!(check_host_allowed("api.example.com", &list).is_ok());
    assert!(check_host_allowed("sub.api.example.com", &list).is_ok());
    assert!(check_host_allowed("example.com", &list).is_err());
}

#[test]
fn loopback_needs_exact() {
    let list = vec!["*.example.com".to_string()];
    // 127.0.0.1 doesn't match *.example.com at all, so it's denied.
    assert!(check_host_allowed("127.0.0.1", &list).is_err());

    // Even if a wildcard DID somehow match a private IP, it would be denied.
    // Test with an explicit exact entry.
    let list2 = vec!["127.0.0.1".to_string()];
    assert!(check_host_allowed("127.0.0.1", &list2).is_ok());
}

#[test]
fn scheme_rejection() {
    let list = vec!["evil.example.com".to_string()];
    assert!(check_url_allowed("ftp://evil.example.com/", &list).is_err());
    assert!(check_url_allowed("file:///etc/passwd", &list).is_err());
}

#[test]
fn empty_allowlist_denies_all() {
    let list: Vec<String> = Vec::new();
    assert!(check_host_allowed("api.example.com", &list).is_err());
}

#[test]
fn private_ip_wildcard_denied() {
    // A wildcard that would match (if the host were named that) still
    // can't reach private IPs — the SSRF guard requires exact match.
    let list = vec!["*".to_string()];
    assert!(check_host_allowed("10.0.0.1", &list).is_err());
    assert!(check_host_allowed("192.168.1.1", &list).is_err());
    assert!(check_host_allowed("172.16.0.1", &list).is_err());
    assert!(check_host_allowed("169.254.1.1", &list).is_err());
    assert!(check_host_allowed("public.example.com", &list).is_ok());
}

#[test]
fn ipv6_loopback() {
    let list = vec!["::1".to_string()];
    assert!(check_host_allowed("::1", &list).is_ok());

    let list2 = vec!["*".to_string()];
    assert!(check_host_allowed("::1", &list2).is_err());
}

#[test]
fn parse_url_ipv6_with_port() {
    let parsed = parse_url("http://[::1]:8080/path").unwrap();
    assert_eq!(parsed.host, "::1");
}

#[test]
fn parse_url_hostname_with_port() {
    let parsed = parse_url("https://api.example.com:443/path?q=1").unwrap();
    assert_eq!(parsed.host, "api.example.com");
}

// ── Finding 2c: non-canonical IP form bypass ─────────────────────────────

#[test]
fn noncanonical_ipv4_decimal_treated_as_private() {
    // 2130706433 == 127.0.0.1 — a wildcard must NOT be tricked into reaching
    // it via the decimal form.
    let list = vec!["*".to_string()];
    assert!(
        check_host_allowed("2130706433", &list).is_err(),
        "decimal-encoded loopback must be recognised as private"
    );
    // 2852039166 == 169.254.169.254 (cloud IMDS).
    assert!(
        check_host_allowed("2852039166", &list).is_err(),
        "decimal-encoded link-local (IMDS) must be recognised as private"
    );
}

#[test]
fn noncanonical_ipv4_hex_treated_as_private() {
    let list = vec!["*".to_string()];
    assert!(
        check_host_allowed("0x7f000001", &list).is_err(),
        "hex-encoded loopback must be recognised as private"
    );
    assert!(
        check_host_allowed("0xA9FEA9FE", &list).is_err(),
        "hex-encoded IMDS must be recognised as private"
    );
}

#[test]
fn ipv4_mapped_ipv6_treated_as_private() {
    let list = vec!["*".to_string()];
    // ::ffff:169.254.169.254 folds down to the IPv4 link-local range.
    assert!(
        check_host_allowed("::ffff:169.254.169.254", &list).is_err(),
        "IPv4-mapped IPv6 IMDS must be recognised as private"
    );
    // Bracketed hex form as it would appear parsed from a URL authority.
    let parsed = parse_url("http://[::ffff:a9fe:a9fe]/").unwrap();
    assert!(
        check_host_allowed(&parsed.host, &list).is_err(),
        "IPv4-mapped IPv6 (hex) IMDS must be recognised as private"
    );
}

#[test]
fn ipv6_unique_local_and_linklocal_private() {
    let list = vec!["*".to_string()];
    assert!(check_host_allowed("fc00::1", &list).is_err());
    assert!(check_host_allowed("fe80::1", &list).is_err());
}

// ── Finding 2c: DNS-resolved SSRF guard ──────────────────────────────────

#[tokio::test]
async fn resolved_guard_rejects_hostname_resolving_to_loopback() {
    // `localhost` resolves to a loopback IP; a wildcard allowlist must not let
    // a guest reach it via the DNS-resolved check even though the literal
    // string passes the wildcard match.
    let list = vec!["*".to_string()];
    let r = check_url_allowed_resolved("http://localhost/", &list).await;
    assert!(
        r.is_err(),
        "hostname resolving to loopback must be rejected by the resolved guard"
    );
}

#[tokio::test]
async fn resolved_guard_allows_exact_loopback_entry() {
    // An operator explicitly allowing 127.0.0.1 (exact entry) is honoured.
    let list = vec!["127.0.0.1".to_string()];
    let r = check_url_allowed_resolved("http://127.0.0.1:8080/x", &list).await;
    assert!(r.is_ok(), "exact loopback allowlist entry must be honoured");
}

// ── Finding 2c: DNS-rebind pin — ResolvedPin return contract ──────────────

#[tokio::test]
async fn resolved_guard_exact_loopback_returns_empty_pin() {
    // The exact-allowlist-match path does NOT resolve DNS, so it returns an
    // empty pin set (nothing to rebind) with the URL's host+port carried
    // through for the caller.
    let list = vec!["127.0.0.1".to_string()];
    let pin = check_url_allowed_resolved("http://127.0.0.1:8080/x", &list)
        .await
        .expect("exact loopback entry honoured");
    assert_eq!(pin.host, "127.0.0.1");
    assert_eq!(pin.port, 8080);
    assert!(
        pin.pinned_ips.is_empty(),
        "exact-allowlist path performs no DNS resolution, so nothing is pinned"
    );
}

#[tokio::test]
async fn resolved_guard_pins_resolved_public_ip() {
    // A wildcard-allowed public literal resolves to itself; the guard must
    // return that exact IP as the pin so curl connects to what was validated.
    // Using a literal IP avoids any live-DNS dependency: `lookup_host` on a
    // literal returns the literal.
    let list = vec!["*".to_string()];
    let pin = check_url_allowed_resolved("http://93.184.216.34:80/", &list)
        .await
        .expect("public literal must pass the resolved guard");
    assert_eq!(pin.host, "93.184.216.34");
    assert_eq!(pin.port, 80);
    assert_eq!(
        pin.pinned_ips,
        vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
        "the validated IP must be returned for pinning"
    );
}

#[test]
fn parse_url_defaults_port_from_scheme() {
    assert_eq!(parse_url("http://example.com/").unwrap().port, 80);
    assert_eq!(parse_url("https://example.com/").unwrap().port, 443);
    assert_eq!(parse_url("http://example.com:8080/x").unwrap().port, 8080);
    assert_eq!(parse_url("https://[::1]:9000/x").unwrap().port, 9000);
    assert_eq!(parse_url("http://[::1]/x").unwrap().port, 80);
}

// ── Finding 2c: inet_aton octal / short IPv4 forms in canonicalize_ip ──────

#[test]
fn canonicalize_octal_per_octet_forms() {
    // Leading `0` on a dotted component means octal (inet_aton). 0177 == 127.
    assert_eq!(
        canonicalize_ip("0177.0.0.1"),
        Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
        "octal 0177 must canonicalize to 127"
    );
    // Full octal loopback.
    assert_eq!(
        canonicalize_ip("0177.0.0.01"),
        Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)))
    );
    // Octal IMDS link-local: 0251=169, 0376=254.
    assert_eq!(
        canonicalize_ip("0251.0376.0251.0376"),
        Some(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)))
    );
}

#[test]
fn canonicalize_hex_per_octet_forms() {
    // Hex per-octet (leading 0x on a dotted component). 0x7f == 127.
    assert_eq!(
        canonicalize_ip("0x7f.0.0.1"),
        Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)))
    );
    assert_eq!(
        canonicalize_ip("0xA9.0xFE.0xA9.0xFE"),
        Some(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)))
    );
}

#[test]
fn canonicalize_short_dotted_forms() {
    // 2-component: last absorbs low 24 bits. 127.1 == 127.0.0.1.
    assert_eq!(
        canonicalize_ip("127.1"),
        Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
        "127.1 must expand to 127.0.0.1"
    );
    // 3-component: last absorbs low 16 bits. 192.168.1 == 192.168.0.1.
    assert_eq!(
        canonicalize_ip("192.168.1"),
        Some(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))),
        "192.168.1 must expand to 192.168.0.1"
    );
    // 2-component where the last absorbs more than one byte: 10.65535 =>
    // 10.0.255.255.
    assert_eq!(
        canonicalize_ip("10.65535"),
        Some(IpAddr::V4(Ipv4Addr::new(10, 0, 255, 255)))
    );
}

#[test]
fn canonicalize_rejects_malformed_inet_aton() {
    // Octal digit out of range (8 is not an octal digit).
    assert_eq!(canonicalize_ip("08.0.0.1"), None);
    // Component > 255 in a non-terminal position.
    assert_eq!(canonicalize_ip("256.0.0.1"), None);
    // Terminal component too large for the bytes it must absorb (a.b => b is
    // 24 bits max; 2^24 overflows).
    assert_eq!(canonicalize_ip("127.16777216"), None);
    // Too many components.
    assert_eq!(canonicalize_ip("1.2.3.4.5"), None);
    // Trailing dot.
    assert_eq!(canonicalize_ip("127.0.0.1."), None);
    // Not an IP at all.
    assert_eq!(canonicalize_ip("api.example.com"), None);
}

#[test]
fn octal_and_short_forms_rejected_by_ssrf_guard() {
    // End-to-end: these non-canonical forms resolving to private/loopback
    // addresses must be REJECTED under a wildcard allowlist (they were NOT
    // recognised as IP literals before this fix).
    let list = vec!["*".to_string()];
    assert!(
        check_host_allowed("0177.0.0.1", &list).is_err(),
        "octal loopback must be blocked"
    );
    assert!(
        check_host_allowed("127.1", &list).is_err(),
        "short-form loopback must be blocked"
    );
    assert!(
        check_host_allowed("192.168.1", &list).is_err(),
        "short-form private must be blocked"
    );
    assert!(
        check_host_allowed("0x7f.0.0.1", &list).is_err(),
        "hex-per-octet loopback must be blocked"
    );
    assert!(
        check_host_allowed("0251.0376.0251.0376", &list).is_err(),
        "octal IMDS must be blocked"
    );
    // And via check_url_allowed (URL wrapper), too.
    assert!(check_url_allowed("http://127.1/", &list).is_err());
}
