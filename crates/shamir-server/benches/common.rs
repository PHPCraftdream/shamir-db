//! Bench-side mirror of `tests/common/mod.rs`.
//!
//! Cargo `[[bench]]` targets cannot reach into the `tests/` directory
//! (they are not part of the test-target dep graph). The canonical
//! helper lives at `crates/shamir-server/tests/common/mod.rs` and is
//! used by integration tests; this file is a deliberate, minimal mirror
//! so the SCRAM connect / resume bench in `wire_latencies.rs` can spawn
//! a live server with the same fixture shape.
//!
//! Keep this file in lock-step with `tests/common/mod.rs`. The
//! duplication is intentional and documented — wrapping the helper in a
//! `[features]`-gated `pub mod` inside the lib would widen the public
//! surface for what is purely a test-side fixture.

#![allow(dead_code)]

use std::path::PathBuf;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig,
    ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};

pub fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

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

pub async fn spawn_ephemeral(temp: &TempDir, admin_password: &[u8]) -> ServerHandle {
    spawn_with_password(temp, admin_password, "127.0.0.1:0").await
}
