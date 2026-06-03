//! TLS 1.3 wiring per TRANSPORT_TCP §3.
//!
//! - Server: self-signed cert generation (rcgen) + ServerConfig builder.
//! - Client: ClientConfig that DOES NOT verify cert chains via CA — identity
//!   is checked at the protocol layer via Ed25519 pin (spec §6).
//! - Both: TLS exporter extraction per RFC 9266 with label
//!   `EXPORTER-ShamirDB-AUTH-v1`.

use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, ServerConfig};
use std::sync::Arc;
use zeroize::Zeroizing;

/// TLS exporter label per spec §4.2 / RFC 9266.
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-ShamirDB-AUTH-v1";
/// Empty exporter context.
pub const EXPORTER_CONTEXT: &[u8] = b"";

/// Generate a fresh self-signed server cert + key (ECDSA P-256 by default).
///
/// Returns `(cert_pem, key_pem)` — caller persists for reuse across restarts
/// to avoid breaking session_id continuity (sessions are in-memory anyway,
/// so cert rotation has no protocol impact in v1). The private-key PEM is
/// returned inside `Zeroizing` so the caller can zeroize it after use.
pub fn generate_self_signed_server_cert(
    subject_alt_names: Vec<String>,
) -> Result<(String, Zeroizing<String>), Box<dyn std::error::Error + Send + Sync>> {
    let key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(subject_alt_names)?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "shamir-db");
    let cert = params.self_signed(&key)?;
    Ok((cert.pem(), Zeroizing::new(key.serialize_pem())))
}

/// Build a TLS 1.3 server config from PEM cert + key.
pub fn make_server_config_from_pem(
    cert_pem: &str,
    key_pem: &str,
) -> Result<Arc<ServerConfig>, Box<dyn std::error::Error + Send + Sync>> {
    let certs = rustls_pemfile::certs(&mut cert_pem.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    let key_pem_bytes = Zeroizing::new(key_pem.as_bytes().to_vec());
    let mut slice = key_pem_bytes.as_slice();
    let mut key_iter = rustls_pemfile::pkcs8_private_keys(&mut slice);
    let key = key_iter.next().ok_or("no PKCS8 key in PEM")??;
    let key = PrivateKeyDer::Pkcs8(key);

    // TLS 1.3 ONLY (TRANSPORT_TCP §3.1 NORMATIVE).
    let cfg = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(Arc::new(cfg))
}

/// Build a TLS 1.3 client config that **accepts any cert** — identity is
/// instead pinned by the application via Ed25519 (spec §6.3 +
/// TRANSPORT_TCP §3.3). TLS 1.3 only — no fallback to TLS 1.2 (§3.1).
pub fn make_client_config_no_ca() -> Arc<ClientConfig> {
    let cfg = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCaVerify))
        .with_no_client_auth();
    Arc::new(cfg)
}

/// Extract the 32-byte TLS exporter for spec §4.2 channel binding.
///
/// Generic over `T: rustls::ConnectionTrait` (covers both client and server
/// post-handshake states). Returns `None` if the handshake is not yet
/// completed or if the cipher suite does not support exporter (TLS 1.3
/// always supports it).
pub fn extract_tls_exporter(connection: &impl ConnectionExporter) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    connection
        .export_keying_material(&mut out, EXPORTER_LABEL, Some(EXPORTER_CONTEXT))
        .ok()?;
    Some(out)
}

/// Trait that tokio-rustls + rustls connections both implement to expose
/// `export_keying_material`. Avoids importing rustls's stable trait, which
/// would tightly couple us to a single rustls version.
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
        conn.export_keying_material(&mut out[..], label, context)?;
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
        conn.export_keying_material(&mut out[..], label, context)?;
        Ok(())
    }
}

/// `ServerCertVerifier` that accepts any cert — identity is pinned out-of-band
/// at the application layer.
#[derive(Debug)]
struct NoCaVerify;

impl rustls::client::danger::ServerCertVerifier for NoCaVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}
