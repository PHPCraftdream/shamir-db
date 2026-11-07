//! CertificateVerify proof-of-possession regressions for RV-TRANSPORT-TCP-1257DB79.
use crate::tls::{
    verifier_for_tests, AppAuthenticatedServerVerifier, AppLayerServerAuth, ConnectionExporter,
};
use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::aws_lc_rs;
use rustls::internal::msgs::codec::Codec;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::sign::SigningKey;
use rustls::SignatureScheme;
use std::sync::Arc;

fn key_material(key: &rcgen::KeyPair) -> PrivateKeyDer<'static> {
    PrivateKeyDer::try_from(key.serialize_der()).expect("PKCS8 key")
}

fn signing_fixture() -> (
    AppAuthenticatedServerVerifier,
    CertificateDer<'static>,
    Arc<dyn SigningKey>,
    Vec<SignatureScheme>,
) {
    let provider = Arc::new(aws_lc_rs::default_provider());
    let key = rcgen::KeyPair::generate().expect("rcgen signing key");
    let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
        .expect("params")
        .self_signed(&key)
        .expect("cert");
    let signing_key =
        aws_lc_rs::sign::any_supported_type(&key_material(&key)).expect("signing key");
    let schemes = provider
        .signature_verification_algorithms
        .supported_schemes();
    (
        verifier_for_tests(provider),
        cert.der().clone(),
        signing_key,
        schemes,
    )
}

fn dss(scheme: SignatureScheme, sig: Vec<u8>) -> rustls::DigitallySignedStruct {
    let mut encoded = Vec::with_capacity(4 + sig.len());
    encoded.extend_from_slice(&(u16::from(scheme)).to_be_bytes());
    encoded.extend_from_slice(&(sig.len() as u16).to_be_bytes());
    encoded.extend_from_slice(&sig);
    rustls::DigitallySignedStruct::read_bytes(&encoded).expect("well-formed signature structure")
}

#[test]
fn rv_transport_tcp_1257db79_verifier_checks_tls13_signature_and_provider_schemes() {
    let (verifier, cert, signing_key, schemes) = signing_fixture();
    let signer = signing_key
        .choose_scheme(&schemes)
        .expect("compatible scheme");
    let message = b"TLS 1.3 CertificateVerify input";
    let signature = signer.sign(message).expect("signature");
    let signed = dss(signer.scheme(), signature);
    assert!(verifier
        .verify_tls13_signature(message, &cert, &signed)
        .is_ok());
    assert!(verifier
        .verify_tls13_signature(b"tampered", &cert, &signed)
        .is_err());
    let (_, _, key_b, _) = signing_fixture();
    let signer_b = key_b.choose_scheme(&schemes).expect("compatible scheme");
    let mismatched = dss(
        signer_b.scheme(),
        signer_b.sign(message).expect("signature"),
    );
    assert!(verifier
        .verify_tls13_signature(message, &cert, &mismatched)
        .is_err());
    assert_eq!(verifier.supported_verify_schemes(), schemes);
    let name = ServerName::try_from("different.example").expect("server name");
    assert!(verifier
        .verify_server_cert(&cert, &[], &name, &[], UnixTime::now())
        .is_ok());
    assert!(verifier
        .verify_tls12_signature(message, &cert, &signed)
        .is_err());
    let unsupported = dss(SignatureScheme::RSA_PKCS1_SHA1, vec![0; 16]);
    assert!(verifier
        .verify_tls13_signature(message, &cert, &unsupported)
        .is_err());
}

#[test]
fn rv_transport_tcp_d1882068_exporter_error_is_preserved() {
    struct FailingExporter;
    impl ConnectionExporter for FailingExporter {
        fn export_keying_material(
            &self,
            _out: &mut [u8],
            _label: &[u8],
            _context: Option<&[u8]>,
        ) -> Result<(), rustls::Error> {
            Err(rustls::Error::General("injected exporter failure".into()))
        }
    }
    // FAILS IF extract_tls_exporter is reverted to the Option-collapsing
    // helper — the underlying rustls error cause would be erased.
    let err = crate::tls::extract_tls_exporter(&FailingExporter).unwrap_err();
    assert!(
        matches!(err, rustls::Error::General(ref m) if m.contains("injected exporter failure")),
        "exporter error cause must be preserved, got {err:?}"
    );
}

#[test]
fn rv_transport_tcp_1257db79_app_layer_opt_in_builds_config() {
    assert!(
        crate::tls::make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound())
            .is_ok()
    );
}
