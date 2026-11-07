//! Verify TRANSPORT_TCP §3.1 NORMATIVE: server accepts ONLY TLS 1.3.
//!
//! A client offering TLS 1.2 only must be rejected. Both server and client
//! configs from `shamir-transport-tcp::tls` MUST refuse to negotiate any
//! version other than TLS 1.3.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::aws_lc_rs::{default_provider, sign::any_supported_type};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::ResolvesServerCert;
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme};
use shamir_transport_tcp::tls::{
    generate_self_signed_server_cert, make_client_config_app_authenticated,
    make_server_config_from_pem, AppLayerServerAuth,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

fn install_provider_once() {
    let _ = default_provider().install_default();
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
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key_pem_bytes = key_pem.as_bytes().to_vec();
    let key = PrivateKeyDer::Pkcs8(
        PrivatePkcs8KeyDer::pem_slice_iter(key_pem_bytes.as_slice())
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

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ED25519,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PKCS1_SHA256,
        ]
    }
}

/// TRANSPORT_TCP §3.1 NORMATIVE: server MUST refuse a TLS 1.2-only client.
#[tokio::test]
async fn server_refuses_tls12_only_client_per_transport_tcp_3_1() {
    install_provider_once();

    let (cert_pem, key_pem) = generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
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
    let server_name = ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let client_result = connector.connect(server_name, tcp).await;

    assert!(
        client_result.is_err(),
        "client offering TLS 1.2 must be rejected by TLS 1.3-only server"
    );
    let server_result = server_task.await.unwrap();
    assert!(server_result.is_err());
}

#[tokio::test]
async fn rv_transport_tcp_11f795d1_tls12_server_is_rejected_at_version_negotiation() {
    install_provider_once();
    let (cert_pem, key_pem) = generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
    let server_cfg = tls12_only_server_config(&cert_pem, &key_pem);
    let client_cfg =
        make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(server_cfg);
    let server_task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        acceptor.accept(tcp).await
    });
    let connector = TlsConnector::from(client_cfg);
    let tcp = TcpStream::connect(addr).await.unwrap();
    let client_result = connector
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await;
    // Discriminator: the failure must be the version-negotiation alert
    // ("protocolversion"). An accidental TLS12-enablement regression surfaces
    // later as a HandshakeFailure/signature-stage alert, which does NOT
    // contain "protocolversion"; a reverted client version pin fails here.
    let err = client_result.unwrap_err();
    let normalized: String = err
        .to_string()
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(
        normalized.contains("protocolversion"),
        "expected a version-negotiation-stage alert, got: {err}"
    );
    let server_result = server_task.await.expect("server task");
    assert!(
        server_result.is_err(),
        "TLS 1.2-only server must reject TLS 1.3 ClientHello"
    );
}

/// TRANSPORT_TCP §3.1 NORMATIVE: client MUST refuse a TLS 1.2-only server.
#[tokio::test]
async fn client_refuses_tls12_only_server_per_transport_tcp_3_1() {
    install_provider_once();

    let (cert_pem, key_pem) = generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
    let server_cfg = tls12_only_server_config(&cert_pem, &key_pem);
    let client_cfg =
        make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound()).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(server_cfg);

    let server_task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        acceptor.accept(tcp).await
    });

    let connector = TlsConnector::from(client_cfg);
    let server_name = ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let client_result = connector.connect(server_name, tcp).await;

    assert!(
        client_result.is_err(),
        "client must refuse to negotiate TLS 1.2 with a TLS 1.2-only server"
    );
    let _ = server_task.await;
}

#[tokio::test]
async fn rv_transport_tcp_1257db79_mismatched_certificate_and_signing_key_fail_handshake() {
    #[derive(Debug)]
    struct FixedResolver(Arc<CertifiedKey>);
    impl ResolvesServerCert for FixedResolver {
        fn resolve(&self, _hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
            Some(self.0.clone())
        }
    }
    install_provider_once();
    let (cert_pem, _cert_key_pem) =
        generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
    let other = rcgen::KeyPair::generate().unwrap();
    let certs = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(other.serialize_der()));
    let signing = any_supported_type(&key_der).unwrap();
    let certified = Arc::new(CertifiedKey::new(certs, signing));
    let provider = Arc::new(default_provider());
    let server = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(FixedResolver(certified)));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server));
    let task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        acceptor.accept(tcp).await
    });
    let cfg = make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound()).unwrap();
    let result = TlsConnector::from(cfg)
        .connect(
            ServerName::try_from("localhost").unwrap(),
            TcpStream::connect(addr).await.unwrap(),
        )
        .await;
    assert!(
        result.is_err(),
        "certificate A signed with key B must fail CertificateVerify"
    );
    assert!(task.await.unwrap().is_err());
}
