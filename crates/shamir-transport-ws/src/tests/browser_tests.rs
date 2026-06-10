use crate::browser::{validate_origin, BrowserOriginPolicy, OriginRejected};

#[test]
fn empty_policy_rejects_everything() {
    let p = BrowserOriginPolicy::empty();
    assert!(matches!(
        validate_origin(&p, Some("https://example.com")),
        Err(OriginRejected::NotAllowed(_))
    ));
}

#[test]
fn missing_origin_always_rejected() {
    let p = BrowserOriginPolicy::allow(["https://example.com"]);
    assert_eq!(validate_origin(&p, None), Err(OriginRejected::Missing));
}

#[test]
fn exact_match_accepted() {
    let p = BrowserOriginPolicy::allow(["https://app.example.com"]);
    assert!(validate_origin(&p, Some("https://app.example.com")).is_ok());
}

#[test]
fn exact_match_rejects_different_scheme() {
    let p = BrowserOriginPolicy::allow(["https://app.example.com"]);
    assert!(validate_origin(&p, Some("http://app.example.com")).is_err());
}

#[test]
fn exact_match_rejects_different_host() {
    let p = BrowserOriginPolicy::allow(["https://app.example.com"]);
    assert!(validate_origin(&p, Some("https://evil.example.com")).is_err());
}

#[test]
fn wildcard_matches_one_subdomain() {
    let p = BrowserOriginPolicy::allow(["https://*.example.com"]);
    assert!(validate_origin(&p, Some("https://app.example.com")).is_ok());
    assert!(validate_origin(&p, Some("https://www.example.com")).is_ok());
}

#[test]
fn wildcard_does_not_match_apex() {
    let p = BrowserOriginPolicy::allow(["https://*.example.com"]);
    assert!(validate_origin(&p, Some("https://example.com")).is_err());
}

#[test]
fn wildcard_does_not_match_deeper_subdomain() {
    // Spec wildcard is single-level (one subdomain).
    let p = BrowserOriginPolicy::allow(["https://*.example.com"]);
    assert!(validate_origin(&p, Some("https://a.b.example.com")).is_err());
}

#[test]
fn wildcard_rejects_different_apex() {
    let p = BrowserOriginPolicy::allow(["https://*.example.com"]);
    assert!(validate_origin(&p, Some("https://app.evil.com")).is_err());
}

#[test]
fn multiple_patterns_any_match() {
    let p = BrowserOriginPolicy::allow([
        "https://app.example.com",
        "https://*.beta.example.com",
        "https://internal.corp",
    ]);
    assert!(validate_origin(&p, Some("https://app.example.com")).is_ok());
    assert!(validate_origin(&p, Some("https://x.beta.example.com")).is_ok());
    assert!(validate_origin(&p, Some("https://internal.corp")).is_ok());
    assert!(validate_origin(&p, Some("https://other.com")).is_err());
}
