//! Verify TRANSPORT_TCP §3.1 NORMATIVE: server accepts ONLY TLS 1.3.
//!
//! A client offering TLS 1.2 only must be rejected. Both server and client
//! configs from `shamir-transport-tcp::tls` MUST refuse to negotiate any
//! version other than TLS 1.3.

use std::sync::Arc;

use rustls::{ClientConfig, ServerConfig};
use shamir_transport_tcp::tls::{
    generate_self_signed_server_cert, make_client_config_no_ca, make_server_config_from_pem,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

fn install_provider_once() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// Build a TLS 1.2-only client config that accepts any cert. Used to
/// confirm that our server (TLS 1.3 only) refuses to handshake with it.
fn tls12_only_client_config() -> Arc<ClientConfig> {
    let cfg = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
        .with_no_client_auth();
    Arc::new(cfg)
}

/// Build a TLS 1.2-only server config. Used to confirm our client (TLS 1.3
/// only) refuses to handshake with it.
fn tls12_only_server_config(cert_pem: &str, key_pem: &str) -> Arc<ServerConfig> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .collect();
    let key_pem_bytes = key_pem.as_bytes().to_vec();
    let mut slice = key_pem_bytes.as_slice();
    let key = PrivateKeyDer::Pkcs8(
        rustls_pemfile::pkcs8_private_keys(&mut slice)
            .next()
            .unwrap()
            .unwrap(),
    );
    let cfg = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    Arc::new(cfg)
}

#[derive(Debug)]
struct AcceptAnyCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
        ]
    }
}

/// TRANSPORT_TCP §3.1 NORMATIVE: server MUST refuse a TLS 1.2-only client.
#[tokio::test]
async fn server_refuses_tls12_only_client_per_transport_tcp_3_1() {
    install_provider_once();

    let (cert_pem, key_pem) =
        generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
    let server_cfg = make_server_config_from_pem(&cert_pem, &key_pem).unwrap();
    let client_cfg = tls12_only_client_config();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(server_cfg);

    let server_task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        // TLS handshake MUST fail (server rejects TLS 1.2 ClientHello).
        acceptor.accept(tcp).await
    });

    let connector = TlsConnector::from(client_cfg);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let client_result = connector.connect(server_name, tcp).await;

    assert!(
        client_result.is_err(),
        "client offering TLS 1.2 must be rejected by TLS 1.3-only server"
    );
    let server_result = server_task.await.unwrap();
    assert!(server_result.is_err());
}

/// TRANSPORT_TCP §3.1 NORMATIVE: client MUST refuse a TLS 1.2-only server.
#[tokio::test]
async fn client_refuses_tls12_only_server_per_transport_tcp_3_1() {
    install_provider_once();

    let (cert_pem, key_pem) =
        generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
    let server_cfg = tls12_only_server_config(&cert_pem, &key_pem);
    let client_cfg = make_client_config_no_ca();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(server_cfg);

    let server_task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        acceptor.accept(tcp).await
    });

    let connector = TlsConnector::from(client_cfg);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let client_result = connector.connect(server_name, tcp).await;

    assert!(
        client_result.is_err(),
        "client must refuse to negotiate TLS 1.2 with a TLS 1.2-only server"
    );
    let _ = server_task.await;
}
