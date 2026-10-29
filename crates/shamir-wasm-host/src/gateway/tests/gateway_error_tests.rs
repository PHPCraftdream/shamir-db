//! Tests for the typed gateway errors (RV-WASM-HOST-DF4AD1DA).
//!
//! Each asserted guard error is produced by production guard code; the literal
//! private-IP check does not exercise DNS-resolved private-address rejection.

use std::error::Error;

use crate::gateway::net_gateway::{
    check_host_allowed, check_url_allowed, check_url_allowed_resolved, NetGatewayError,
};

// DNS resolution errors preserve their originating I/O source. The embedded NUL
// is rejected by the system resolver before any DNS/network request occurs.
#[tokio::test]
async fn rv_wasm_host_df4ad1da_resolved_dns_error_retains_io_source() {
    let err = check_url_allowed_resolved("http://bad\0name/", &["*".to_string()])
        .await
        .unwrap_err();
    assert!(matches!(err, NetGatewayError::Dns { .. }));
    assert!(err
        .source()
        .unwrap()
        .downcast_ref::<std::io::Error>()
        .is_some());
}

#[test]
fn rv_wasm_host_df4ad1da_typed_gateway_errors() {
    // NetGatewayError discriminates allowlist/SSRF denials from other
    // failure classes. Each error below comes straight from the guards.

    // Empty allowlist → guard denial.
    let denied = check_host_allowed("h", &[]).unwrap_err();
    assert!(matches!(denied, NetGatewayError::NotAllowed(_)));
    assert_eq!(denied.to_string(), "egress to h not allowed");

    // Wildcard covering a private IP → SSRF guard denial.
    let private = check_host_allowed("127.0.0.1", &["*".to_string()]).unwrap_err();
    assert!(matches!(private, NetGatewayError::NotAllowed(_)));
    assert_eq!(
        private.to_string(),
        "egress to 127.0.0.1 not allowed (IP policy)"
    );

    // Bad scheme → guard denial.
    let bad_scheme = check_url_allowed("ftp://x/", &["*".to_string()]).unwrap_err();
    assert!(matches!(bad_scheme, NetGatewayError::NotAllowed(_)));
    assert_eq!(
        bad_scheme.to_string(),
        "egress: scheme 'ftp' not allowed (only http/https)"
    );

    // Bad port → guard denial.
    let bad_port = check_url_allowed("http://x:notaport/", &["*".to_string()]).unwrap_err();
    assert!(matches!(bad_port, NetGatewayError::NotAllowed(_)));
    assert_eq!(
        bad_port.to_string(),
        "egress: invalid port in URL: http://x:notaport/"
    );
}
