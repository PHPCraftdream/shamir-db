use super::{certificate_fields, validate_http_tls};
use crate::boot::config::TlsConfig;
use std::net::SocketAddr;

fn write_certificate(pem: &str, key_pem: &str) -> (tempfile::TempDir, TlsConfig) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cert.pem");
    std::fs::write(&path, pem).unwrap();
    let key_path = dir.path().join("key.pem");
    std::fs::write(&key_path, key_pem).unwrap();
    (
        dir,
        TlsConfig {
            cert_path: path,
            key_path,
        },
    )
}

#[test]
fn generated_self_issued_certificate_is_rejected_remotely_unless_allowed() {
    let (cert, key) =
        shamir_transport_tcp::tls::generate_self_signed_server_cert(vec!["localhost".into()])
            .unwrap();
    let (_dir, config) = write_certificate(&cert, key.as_str());
    let remote: SocketAddr = "192.0.2.1:443".parse().unwrap();
    assert!(validate_http_tls(&config, remote, false).is_err());
    assert!(validate_http_tls(&config, remote, true).is_ok());
    assert!(validate_http_tls(&config, "127.0.0.1:443".parse().unwrap(), false).is_ok());
}

#[test]
fn distinct_issuer_and_subject_are_not_refused() {
    let names = [0x30, 0x00];
    let other = [0x30, 0x02, 0x31, 0x00];
    let mut tbs_content = vec![0x02, 0x01, 0x01, 0x30, 0x00];
    tbs_content.extend_from_slice(&other);
    tbs_content.extend_from_slice(&[0x30, 0x00]);
    tbs_content.extend_from_slice(&names);
    tbs_content.extend_from_slice(&[0x30, 0x00]);
    let mut tbs = vec![0x30, tbs_content.len() as u8];
    tbs.extend_from_slice(&tbs_content);
    let mut der = vec![0x30, (tbs.len() + 2) as u8];
    der.extend_from_slice(&tbs);
    der.extend_from_slice(&[0x30, 0x00]);
    let (issuer, subject, spki) = certificate_fields(&der).unwrap();
    assert_ne!(issuer, subject);
    assert_eq!(spki, &[0x30, 0x00]);
}

#[test]
fn malformed_der_is_rejected_without_panicking() {
    for der in [
        &[][..],
        &[0x30][..],
        &[0x30, 0x80][..],
        &[0x30, 0x82, 0xff, 0xff][..],
    ] {
        assert!(certificate_fields(der).is_none());
    }
}
