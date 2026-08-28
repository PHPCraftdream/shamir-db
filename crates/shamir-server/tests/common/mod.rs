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

/// Local-IPC listener address for `kind: unix` test configs. On Unix, a
/// socket path inside `temp`. On Windows, a Named Pipe name — unique per
/// call via `temp`'s own randomly-generated directory name (there is no
/// filesystem meaning to a pipe name on Windows, so `temp` is reused
/// purely as a source of per-test uniqueness, not a real path).
pub fn ipc_test_addr(temp: &TempDir) -> String {
    #[cfg(unix)]
    {
        temp.path()
            .join("shamir-test.sock")
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(windows)]
    {
        let tag = temp
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("shamir-test");
        format!(r"\\.\pipe\{tag}")
    }
}

/// Build a minimal single-`kind: unix`-listener `Config` rooted at `temp`.
pub fn make_ipc_test_config(temp: &TempDir, addr: &str) -> Config {
    Config {
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Unix,
            addr: addr.to_string(),
            profile: ProfileKind::Plain,
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
        }],
        ..make_test_config(temp, "127.0.0.1:0")
    }
}

/// Spawn a fresh server with a single `kind: unix` local-IPC listener and
/// an `admin` superuser bootstrapped to `password`. Returns the
/// [`ServerHandle`] whose `first_ipc_path()` exposes the bound socket
/// path / pipe name.
///
/// The caller owns `temp` and must keep it alive for the duration of
/// the test — dropping it deletes the data dir (and, on Unix, the socket
/// file) out from under the running server.
pub async fn spawn_ipc_with_password(
    temp: &TempDir,
    admin_password: &[u8],
    addr: &str,
) -> ServerHandle {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let config = make_ipc_test_config(temp, addr);
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_password.to_vec()),
    };
    ServerLauncher { config, bootstrap }
        .launch()
        .await
        .expect("launcher boot")
}

/// Build a minimal single-TCP-listener `Config` with `profile: plain`
/// (no TLS, `binding_mode = 0x00`) rooted at `temp`. Per TRANSPORT_TCP
/// §2.2, `Config::validate` only accepts a `plain` profile on a loopback
/// `addr` — pass e.g. `"127.0.0.1:0"`.
pub fn make_plain_test_config(temp: &TempDir, addr: &str) -> Config {
    Config {
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: addr.to_string(),
            profile: ProfileKind::Plain,
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
        }],
        ..make_test_config(temp, addr)
    }
}

/// Spawn a fresh server with a single `profile: plain` TCP listener and
/// an `admin` superuser bootstrapped to `password`. Returns the
/// [`ServerHandle`] whose `first_tls_exporter_addr()` exposes the bound
/// port (the name is generic despite this listener being plain — it just
/// returns the first bound address).
///
/// The caller owns `temp` and must keep it alive for the duration of
/// the test — dropping it deletes the data dir out from under the
/// running server.
pub async fn spawn_plain_with_password(
    temp: &TempDir,
    admin_password: &[u8],
    addr: &str,
) -> ServerHandle {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let config = make_plain_test_config(temp, addr);
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_password.to_vec()),
    };
    ServerLauncher { config, bootstrap }
        .launch()
        .await
        .expect("launcher boot")
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

/// Spawn a fresh server with an `admin` superuser bootstrapped via
/// [`BootstrapMode::RandomToken`] (default username, default
/// `data_dir/bootstrap_token.txt` path — no override). Returns the
/// [`ServerHandle`] plus the generated token (read back off disk), so the
/// caller can use it as the login password.
///
/// The caller owns `temp` and must keep it alive for the duration of the
/// test — dropping it deletes the data dir out from under the running
/// server.
pub async fn spawn_with_random_token(temp: &TempDir, addr: &str) -> (ServerHandle, String) {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let config = make_test_config(temp, addr);
    let bootstrap = BootstrapMode::RandomToken {
        username: None,
        token_path: None,
    };
    let handle = ServerLauncher { config, bootstrap }
        .launch()
        .await
        .expect("launcher boot");
    let token = std::fs::read_to_string(temp.path().join("bootstrap_token.txt"))
        .expect("bootstrap token file must exist right after boot");
    (handle, token)
}
