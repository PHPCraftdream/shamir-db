use crate::net_gateway::{
    check_host_allowed, check_url_allowed, check_url_allowed_resolved, parse_url,
};

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
