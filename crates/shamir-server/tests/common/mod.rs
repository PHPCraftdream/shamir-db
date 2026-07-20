//! Shared live-server harness for integration tests.
//!
//! Cargo treats every `tests/*.rs` file as its own crate, so any code
//! shared across them must live under `tests/common/mod.rs`. Each test
//! file pulls it in with `mod common;`.
//!
//! The harness is the dedup of ~10 `*_e2e.rs` files that each inlined
//! the same `fast_kdf()` + `make_test_config()` + `ServerLauncher { ...
//! }.launch()` sequence (~30 LOC apiece). The full migration sweep
//! happens in follow-up commits — Stage 1 only migrates a handful of
//! representative files to lock the helper's shape in.
//!
//! The deferred SCRAM connect / resume bench in
//! `benches/wire_latencies.rs` (Group 2) is the second consumer; since
//! `[[bench]]` targets cannot reach into `tests/common/`, the bench
//! mirrors this file locally — see `benches/common.rs` in the Stage-2
//! commit.

// Each test crate uses only a subset of these items — silence the
// per-crate dead-code lint at the module boundary.
#![allow(dead_code)]

use std::path::PathBuf;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig,
    ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};

/// Spec-floor Argon2id parameters — fast enough for tests, real enough
/// that the full KDF code path runs.
pub fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

/// Build a minimal single-TCP-listener `Config` rooted at `temp`.
///
/// `addr` is passed through verbatim — pass `"127.0.0.1:0"` to let the
/// OS pick a free port (recovered via
/// `ServerHandle::first_tls_exporter_addr()`), or a fixed port for
/// benches that need deterministic targets.
pub fn make_test_config(temp: &TempDir, addr: &str) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
    Config {
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        kdf_defaults: fast_kdf(),
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: addr.to_string(),
            profile: ProfileKind::TlsExporter,
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: Default::default(),
        audit: Default::default(),
        observability: ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

/// Spawn a fresh server with an `admin` superuser bootstrapped to
/// `password`. Returns the [`ServerHandle`] handle whose
/// `first_tls_exporter_addr()` exposes the bound port.
///
/// The caller owns `temp` and must keep it alive for the duration of
/// the test — dropping it deletes the data dir out from under the
/// running server.
pub async fn spawn_with_password(
    temp: &TempDir,
    admin_password: &[u8],
    addr: &str,
) -> ServerHandle {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let config = make_test_config(temp, addr);
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_password.to_vec()),
    };
    ServerLauncher { config, bootstrap }
        .launch()
        .await
        .expect("launcher boot")
}

/// Convenience: spawn on an OS-assigned port (`127.0.0.1:0`).
pub async fn spawn_ephemeral(temp: &TempDir, admin_password: &[u8]) -> ServerHandle {
    spawn_with_password(temp, admin_password, "127.0.0.1:0").await
}
