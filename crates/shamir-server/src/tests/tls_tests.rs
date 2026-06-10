use std::fs;

use tempfile::TempDir;

use crate::tls::{load_or_generate, subject_alts_from_addrs, TlsError};

#[test]
fn generates_when_absent() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let dir = TempDir::new().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    let res = load_or_generate(&cert, &key, vec!["localhost".into()]).unwrap();
    assert!(res.generated, "first run must mark generated");
    assert!(cert.exists() && key.exists(), "PEMs persisted on disk");
}

#[test]
fn loads_existing_without_regenerating() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let dir = TempDir::new().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    load_or_generate(&cert, &key, vec!["localhost".into()]).unwrap();
    let cert_bytes = fs::read(&cert).unwrap();
    let key_bytes = fs::read(&key).unwrap();
    let res = load_or_generate(&cert, &key, vec!["localhost".into()]).unwrap();
    assert!(!res.generated, "second run must reuse on-disk PEMs");
    assert_eq!(fs::read(&cert).unwrap(), cert_bytes, "cert untouched");
    assert_eq!(fs::read(&key).unwrap(), key_bytes, "key untouched");
}

#[test]
fn refuses_half_present() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let dir = TempDir::new().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    fs::write(&cert, "garbage").unwrap();
    // `LoadedTls` does not impl Debug (its inner ServerConfig doesn't),
    // so we match instead of `unwrap_err`.
    match load_or_generate(&cert, &key, vec![]) {
        Err(TlsError::Mismatched {
            cert_exists: true,
            key_exists: false,
        }) => {}
        other => panic!("expected Mismatched (cert only); got {:?}", other.is_ok()),
    }
}

#[test]
fn alts_include_listener_ips() {
    let alts = subject_alts_from_addrs(&[
        "127.0.0.1:1".parse().unwrap(),
        "10.0.0.5:1".parse().unwrap(),
        "127.0.0.1:2".parse().unwrap(), // dup IP, second port
    ]);
    assert_eq!(alts, vec!["localhost", "127.0.0.1", "10.0.0.5"]);
}
