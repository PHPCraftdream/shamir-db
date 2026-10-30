//! TLS 1.3 wiring per TRANSPORT_TCP §3.
//!
//! - Server: self-signed cert generation (rcgen) + ServerConfig builder.
//! - Client: ClientConfig skips CA chains/hostname; application identity is
//!   verified with exporter-bound Ed25519 proof (spec §6).
//! - Both: Project-specific TLS exporter extraction with label
//!   `EXPORTER-ShamirDB-AUTH-v1`.
//!
//! A TLS-terminating relay has a different exporter on each leg, so relayed
//! SCRAM/identity proofs fail. This does not protect `auth_init` (username and
//! client nonce) or `client_proof` sent before server authentication.
//! CertificateVerify possession alone is not a MITM defense.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{aws_lc_rs, CryptoProvider};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::PrivatePkcs8KeyDer;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, ServerConfig};
use std::sync::Arc;
use zeroize::Zeroizing;

/// Explicit opt-in to skip CA-chain and hostname validation.
///
/// Callers must fail closed if exporter extraction fails, bind that exporter
/// into the SCRAM exchange, and verify the pinned Ed25519 server identity.
#[derive(Clone, Copy, Debug)]
pub struct AppLayerServerAuth {
    _private: (),
}

impl AppLayerServerAuth {
    /// Opt in when application-level exporter-bound server authentication is
    /// performed before using the connection or resuming a session.
    pub const fn exporter_bound() -> Self {
        Self { _private: () }
    }
}

/// Errors from TLS config / cert material construction.
#[derive(Debug, thiserror::Error)]
pub enum TlsConfigError {
    /// PEM parse failure (cert or key slice was not valid PEM).
    #[error("pem parse: {0}")]
    Pem(#[from] rustls_pki_types::pem::Error),
    /// The key PEM contained no PKCS8 key.
    #[error("no PKCS8 key in PEM")]
    NoKeyInPem,
    /// rustls config build failure (version validation, cert/key mismatch).
    #[error("rustls: {0}")]
    Rustls(#[from] rustls::Error),
    /// Self-signed certificate generation failure.
    #[error("rcgen: {0}")]
    Rcgen(#[from] rcgen::Error),
}

/// Project-specific TLS exporter label (spec §4.2), not the RFC 9266 label.
pub const EXPORTER_LABEL: &[u8] = shamir_connect::common::domain_tags::TLS_EXPORTER_LABEL;
/// Empty exporter context.
pub const EXPORTER_CONTEXT: &[u8] = b"";

/// Generate a fresh self-signed server cert + key (ECDSA P-256 by default).
///
/// This synchronous bootstrap operation is CPU-bound; active async-runtime
/// callers must run it with `tokio::task::spawn_blocking`.
///
/// Returns `(cert_pem, key_pem)` — caller persists for reuse across restarts
/// to avoid breaking session_id continuity (sessions are in-memory anyway,
/// so cert rotation has no protocol impact in v1). The private-key PEM is
/// returned inside `Zeroizing` so the caller can zeroize it after use.
pub fn generate_self_signed_server_cert(
    subject_alt_names: Vec<String>,
) -> Result<(String, Zeroizing<String>), TlsConfigError> {
    let key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(subject_alt_names)?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "shamir-db");
    let cert = params.self_signed(&key)?;
    Ok((cert.pem(), Zeroizing::new(key.serialize_pem())))
}

/// Build a TLS 1.3 server config from PEM cert + key.
///
/// This synchronous bootstrap operation is CPU-bound; active async-runtime
/// callers must run it with `tokio::task::spawn_blocking`.
pub fn make_server_config_from_pem(
    cert_pem: &str,
    key_pem: &str,
) -> Result<Arc<ServerConfig>, TlsConfigError> {
    let certs =
        CertificateDer::pem_slice_iter(cert_pem.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    let key_pem_bytes = Zeroizing::new(key_pem.as_bytes().to_vec());
    let key = PrivatePkcs8KeyDer::pem_slice_iter(key_pem_bytes.as_slice())
        .next()
        .ok_or(TlsConfigError::NoKeyInPem)??;
    let key = PrivateKeyDer::Pkcs8(key);

    // TLS 1.3 ONLY (TRANSPORT_TCP §3.1 NORMATIVE). Explicit provider: never
    // consults (or panics on the absence of) the process-default provider.
    let provider = Arc::new(aws_lc_rs::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(Arc::new(cfg))
}

/// Build a TLS 1.3 client config that **accepts any cert chain and hostname** —
/// the pinned Ed25519 identity is instead verified at the protocol layer
/// (spec §6.3 + TRANSPORT_TCP §3.3). TLS 1.3 only — no fallback to TLS 1.2
/// (§3.1). An explicit call-site token is required for this intentional bypass.
pub fn make_client_config_app_authenticated(
    _auth: AppLayerServerAuth,
) -> Result<Arc<ClientConfig>, TlsConfigError> {
    let provider = Arc::new(aws_lc_rs::default_provider());
    let verifier = Arc::new(AppAuthenticatedServerVerifier {
        provider: provider.clone(),
    });
    let cfg = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

/// Extract the 32-byte TLS exporter for spec §4.2 channel binding.
///
/// Uses the local [`ConnectionExporter`] adapter for tokio-rustls client and
/// server streams. Returns the underlying rustls error so callers can
/// fail closed when the handshake is incomplete or exporter is unavailable.
pub fn extract_tls_exporter(
    connection: &impl ConnectionExporter,
) -> Result<[u8; 32], rustls::Error> {
    let mut out = [0u8; 32];
    connection.export_keying_material(&mut out, EXPORTER_LABEL, Some(EXPORTER_CONTEXT))?;
    Ok(out)
}

/// Local adapter exposing exporter extraction for tokio-rustls client and
/// server streams.
pub trait ConnectionExporter {
    /// Fill `out` with TLS exporter output for `(label, context)`.
    fn export_keying_material(
        &self,
        out: &mut [u8],
        label: &[u8],
        context: Option<&[u8]>,
    ) -> Result<(), rustls::Error>;
}

impl<S> ConnectionExporter for tokio_rustls::server::TlsStream<S> {
    fn export_keying_material(
        &self,
        out: &mut [u8],
        label: &[u8],
        context: Option<&[u8]>,
    ) -> Result<(), rustls::Error> {
        let (_, conn) = self.get_ref();
        conn.export_keying_material(out, label, context)?;
        Ok(())
    }
}

impl<S> ConnectionExporter for tokio_rustls::client::TlsStream<S> {
    fn export_keying_material(
        &self,
        out: &mut [u8],
        label: &[u8],
        context: Option<&[u8]>,
    ) -> Result<(), rustls::Error> {
        let (_, conn) = self.get_ref();
        conn.export_keying_material(out, label, context)?;
        Ok(())
    }
}

/// `ServerCertVerifier` that deliberately skips CA-chain and hostname checks;
/// the caller authenticates the pinned identity out-of-band.
#[derive(Debug)]
pub(crate) struct AppAuthenticatedServerVerifier {
    provider: Arc<CryptoProvider>,
}

#[cfg(test)]
pub(crate) fn verifier_for_tests(provider: Arc<CryptoProvider>) -> AppAuthenticatedServerVerifier {
    AppAuthenticatedServerVerifier { provider }
}

impl ServerCertVerifier for AppAuthenticatedServerVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // Intentionally skip CA-chain and hostname validation; the caller
        // MUST authenticate the pinned Ed25519 identity with TLS exporter proofs.
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
