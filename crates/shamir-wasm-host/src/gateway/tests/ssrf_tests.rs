//! Tests for the IPv4-unspecified (0.0.0.0) SSRF guard
//! (RV-WASM-HOST-E62F8E2C).

use crate::gateway::net_gateway::{check_host_allowed, check_url_allowed};

#[test]
fn rv_wasm_host_e62f8e2c_zero_addr_blocked_by_wildcard() {
    // A wildcard allowlist must NOT reach 0.0.0.0.
    let wildcard = vec!["*".to_string()];
    assert!(check_host_allowed("0.0.0.0", &wildcard).is_err());
    assert!(check_url_allowed("http://0.0.0.0:8080/", &wildcard).is_err());

    // Unspecified addresses are forbidden even with an exact entry.
    let exact = vec!["0.0.0.0".to_string()];
    assert!(check_host_allowed("0.0.0.0", &exact).is_err());

    // Regression guard: IPv6 unspecified stays blocked under a wildcard.
    assert!(check_host_allowed("::", &wildcard).is_err());
}
