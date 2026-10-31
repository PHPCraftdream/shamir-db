//! Regression tests for card RV-TRANSPORT-TCP-0A23323D: TLS config builders
//! are fallible and use an explicit `aws_lc_rs` provider, so they never
//! consult, install, or depend on the process-default provider.

use crate::tls::{
    generate_self_signed_server_cert, make_client_config_app_authenticated,
    make_server_config_from_pem, AppLayerServerAuth, TlsConfigError,
};

use rustls::crypto::CryptoProvider;

/// RV-TRANSPORT-TCP-A38C5274: a mismatched certificate/private key pair is
/// rejected by the TLS config constructor with rustls's discriminating cause.
#[test]
fn rv_transport_tcp_a38c5274_certificate_key_mismatch_is_rejected() {
    let (cert, _) = generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
    let (_, other_key) = generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
    let error = make_server_config_from_pem(&cert, &other_key).unwrap_err();
    match error {
        TlsConfigError::Rustls(rustls::Error::InconsistentKeys(_)) => {}
        other => panic!("expected certificate/key mismatch, got {other:?}"),
    }
}

/// The client config must build Ok WITHOUT installing (or consulting) the
/// process-default provider: with the old implicit
/// `builder_with_protocol_versions` path, the first build auto-installed the
/// process default, so `CryptoProvider::get_default()` became `Some` after
/// construction and this assertion failed. With `builder_with_provider` the
/// global default is never touched.
#[test]
fn rv_transport_tcp_0a23323d_client_config_builds_without_global_provider() {
    assert!(
        CryptoProvider::get_default().is_none(),
        "precondition: no process-default provider installed"
    );
    let cfg = make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound());
    assert!(cfg.is_ok());
    assert!(
        CryptoProvider::get_default().is_none(),
        "config build must not install a process-default provider"
    );
}

/// Garbage PEM with a `-----BEGIN` line but no well-formed section must yield
/// the typed `Pem` parse error (rustls-pki-types reports MissingSectionEnd at
/// EOF), not an opaque `Box<dyn Error>` or a stringly error.
#[test]
fn rv_transport_tcp_0a23323d_server_config_rejects_malformed_pem() {
    let err = match make_server_config_from_pem("-----BEGIN CERTIFICATE-----\nAAAA", "AAAA") {
        Err(e) => e,
        Ok(_) => panic!("malformed pem must be rejected"),
    };
    assert!(matches!(err, TlsConfigError::Pem(_)), "got: {err:?}");
}

/// A valid certificate PEM with no PKCS8 key must yield `NoKeyInPem`.
#[test]
fn rv_transport_tcp_0a23323d_server_config_rejects_cert_only_pem() {
    let (cert_pem, _key_pem) = generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
    let err = match make_server_config_from_pem(&cert_pem, "") {
        Err(e) => e,
        Ok(_) => panic!("cert-only pem must be rejected"),
    };
    assert!(matches!(err, TlsConfigError::NoKeyInPem), "got: {err:?}");
}

/// Both configs build successfully with no ambient provider installed, and
/// construction leaves the global default untouched (same revert-sensitive
/// check as `..._builds_without_global_provider`). NOTE: an explicit
/// "incompatible preinstalled provider" test is impossible here:
/// `CryptoProvider::install_default` writes a process-wide OnceLock that
/// cannot be scoped or reset and would poison every other test in this
/// binary — the explicit `builder_with_provider` path bypasses provider
/// resolution entirely, which this test observes behaviourally via
/// `get_default()` staying `None`.
#[test]
fn rv_transport_tcp_0a23323d_explicit_provider_ignores_ambient_install() {
    assert!(make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound()).is_ok());
    let (cert_pem, key_pem) = generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
    assert!(make_server_config_from_pem(&cert_pem, &key_pem).is_ok());
    assert!(
        CryptoProvider::get_default().is_none(),
        "explicit provider selection must not touch the process default"
    );
}

/// RV-TRANSPORT-TCP-A38C5274: a REAL rustls connection whose TLS 1.3
/// handshake never completes must fail exporter extraction with the
/// SPECIFIC rustls cause, not a catch-all. rustls reports any pre-completion
/// handshake state via `Error::HandshakeNotComplete` (the default
/// `common_state::State::export_keying_material`, rustls 0.23.46).
///
/// Note: tokio-rustls 0.26 only hands out a `client::TlsStream` AFTER a
/// completed handshake (its `MidHandshake` future polls
/// `while session.is_handshaking()`), so an uncompleted tokio-rustls stream
/// is unreachable from user code. The uncompleted connection here is built
/// from the crate's REAL client config (`make_client_config_app_authenticated`)
/// and exercised through the crate's own `ConnectionExporter` seam.
struct UncompletedClientExporter(rustls::ClientConnection);

impl crate::tls::ConnectionExporter for UncompletedClientExporter {
    fn export_keying_material(
        &self,
        out: &mut [u8],
        label: &[u8],
        context: Option<&[u8]>,
    ) -> Result<(), rustls::Error> {
        self.0
            .export_keying_material(out, label, context)
            .map(|_| ())
    }
}

#[test]
fn rv_transport_tcp_a38c5274_uncompleted_handshake_exporter_fails_with_specific_cause() {
    let config =
        make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound()).unwrap();
    let server_name = rustls::pki_types::ServerName::try_from("localhost".to_string()).unwrap();
    let mut conn = rustls::ClientConnection::new(config, server_name).unwrap();

    // Drive the handshake one step for real: emit the ClientHello onto the
    // wire (a plain in-memory sink; rustls write_tls needs a std Write).
    let mut wire = Vec::new();
    let written = conn.write_tls(&mut wire).unwrap();
    assert!(written > 0, "ClientHello must be written");
    assert!(
        conn.is_handshaking(),
        "no server response: still handshaking"
    );

    let err = crate::tls::extract_tls_exporter(&UncompletedClientExporter(conn)).unwrap_err();
    match err {
        rustls::Error::HandshakeNotComplete => {}
        other => panic!("expected HandshakeNotComplete, got {other:?}"),
    }
}
