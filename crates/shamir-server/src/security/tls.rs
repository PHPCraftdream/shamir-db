//! TLS material lifecycle for the production server.
//!
//! On boot we look at the configured `cert_path` / `key_path`:
//!
//! - **both files exist** → load them, build a `tokio_rustls::ServerConfig`.
//! - **neither file exists** → generate a fresh self-signed pair via
//!   `shamir_transport_tcp::tls::generate_self_signed_server_cert`, persist
//!   both PEMs, then load. Subject Alternative Names default to
//!   `localhost` + every distinct IP literal among the configured listeners
//!   so a single self-signed cert covers `127.0.0.1`, the LAN address, etc.
//! - **exactly one file exists** → refuse to start. Mismatched persistence
//!   on disk is a configuration bug, not a state to silently recover from.
//!
//! Identity in this protocol is pinned at the application layer (Ed25519
//! signature inside `auth_ok`), so the X.509 chain is informational only —
//! a self-signed cert is fine for production. The protocol does not need a
//! CA-issued cert for any security property.

use std::fs;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use tokio_rustls::rustls::ServerConfig;

use zeroize::Zeroizing;

use shamir_transport_tcp::tls::{generate_self_signed_server_cert, make_server_config_from_pem};

/// Errors raised by [`load_or_generate`].
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    /// Only one of the two PEM files exists. Refuse rather than silently
    /// regenerate (the existing file may be paired with material the operator
    /// is mid-rotation on).
    #[error(
        "tls material is half-present: cert exists = {cert_exists}, \
         key exists = {key_exists}; both must exist or both must be absent"
    )]
    Mismatched { cert_exists: bool, key_exists: bool },
    /// Filesystem error reading or writing the PEM files.
    #[error("tls io: {0}")]
    Io(#[from] io::Error),
    /// Self-signed generation (rcgen) or PEM parse / rustls build failure.
    #[error("tls build: {0}")]
    Build(String),
}

/// Result of the cert lifecycle: the loaded `ServerConfig` plus a marker
/// telling the caller whether a new pair was generated (so the boot path
/// can log a security-sensitive notice).
pub struct LoadedTls {
    pub server_config: Arc<ServerConfig>,
    pub generated: bool,
}

/// Load existing or generate a fresh PEM pair, then build the TLS 1.3
/// ServerConfig. See module docs for behaviour.
///
/// `subject_alts` should be the union of every host/IP a client might use
/// to reach the server (e.g. `localhost`, `127.0.0.1`, the LAN address).
/// Ignored when both files already exist on disk.
pub fn load_or_generate(
    cert_path: &Path,
    key_path: &Path,
    subject_alts: Vec<String>,
) -> Result<LoadedTls, TlsError> {
    let cert_exists = cert_path.exists();
    let key_exists = key_path.exists();

    let (cert_pem, key_pem, generated) = match (cert_exists, key_exists) {
        (true, true) => {
            let cert = fs::read_to_string(cert_path)?;
            let key = Zeroizing::new(fs::read_to_string(key_path)?);
            (cert, key, false)
        }
        (false, false) => {
            // Generate, persist, then load — keeps the load path uniform.
            if let Some(parent) = cert_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if let Some(parent) = key_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let alts = if subject_alts.is_empty() {
                vec!["localhost".into()]
            } else {
                subject_alts
            };
            let (cert, key) = generate_self_signed_server_cert(alts)
                .map_err(|e| TlsError::Build(e.to_string()))?;
            fs::write(cert_path, &cert)?;
            // Key file: try to set permissions to 0600 on Unix; on Windows
            // there's no concise equivalent and the directory ACL is the
            // operator's responsibility.
            fs::write(key_path, &key)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(key_path)?.permissions();
                perms.set_mode(0o600);
                let _ = fs::set_permissions(key_path, perms);
            }
            (cert, key, true)
        }
        (a, b) => {
            return Err(TlsError::Mismatched {
                cert_exists: a,
                key_exists: b,
            })
        }
    };

    let server_config = make_server_config_from_pem(&cert_pem, &key_pem)
        .map_err(|e| TlsError::Build(e.to_string()))?;
    Ok(LoadedTls {
        server_config,
        generated,
    })
}

/// Convenience: build the SAN list from a slice of bound listener addresses.
/// Always includes `localhost`.
pub fn subject_alts_from_addrs(addrs: &[SocketAddr]) -> Vec<String> {
    let mut out = vec!["localhost".to_string()];
    for a in addrs {
        let ip_str = match a.ip() {
            IpAddr::V4(v) => v.to_string(),
            IpAddr::V6(v) => v.to_string(),
        };
        if !out.iter().any(|s| s == &ip_str) {
            out.push(ip_str);
        }
    }
    out
}
