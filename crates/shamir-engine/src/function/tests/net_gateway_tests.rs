use crate::function::net_gateway::{check_host_allowed, check_url_allowed, parse_url};

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
