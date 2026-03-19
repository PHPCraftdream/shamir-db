//! TLS configuration: self-signed certificate generation.

use std::sync::Arc;
use rcgen::{CertificateParams, KeyPair};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::TlsAcceptor;

/// Generate a self-signed TLS config for the server.
pub fn create_tls_config() -> (TlsAcceptor, Vec<u8>) {
    let key_pair = KeyPair::generate().expect("Failed to generate key pair");
    let mut params = CertificateParams::new(vec!["localhost".to_string()])
        .expect("Failed to create cert params");
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        rcgen::DnValue::Utf8String("ShamirDB".to_string()),
    );

    let cert = params.self_signed(&key_pair).expect("Failed to self-sign cert");
    let cert_der = CertificateDer::from(cert.der().to_vec());
    let cert_pem = cert.pem().into_bytes();
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("Failed to build TLS config");

    (TlsAcceptor::from(Arc::new(config)), cert_pem)
}
