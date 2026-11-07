use crate::client::transport::token::{build_connector, parse_server_name, TokenTrust};
use crate::error::ClientError;

#[test]
fn webpki_roots_connector_builds() {
    assert!(build_connector(&TokenTrust::webpki_roots()).is_ok());
}

#[test]
fn pem_empty_is_rejected() {
    let err = TokenTrust::pem(b"").unwrap_err();
    assert!(matches!(err, ClientError::Tls { .. }));
}

#[test]
fn pem_garbage_is_rejected() {
    let err = TokenTrust::pem(b"not a certificate").unwrap_err();
    assert!(matches!(err, ClientError::Tls { .. }));
}

#[test]
fn garbage_der_fails_to_build() {
    let trust = TokenTrust::der(vec![1u8, 2, 3]);
    assert!(build_connector(&trust).is_err());
}

#[test]
fn empty_custom_roots_fail_to_build() {
    assert!(build_connector(&TokenTrust::CustomRoots(Vec::new())).is_err());
}

#[test]
fn server_name_accepts_dns_and_ip() {
    assert!(parse_server_name("localhost").is_ok());
    assert!(parse_server_name("127.0.0.1").is_ok());
}

#[test]
fn server_name_rejects_invalid() {
    assert!(parse_server_name("bad name!").is_err());
}
