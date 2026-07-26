//! Boot-level regression guard for the F-15 wiring in
//! `server_launcher.rs`: `security.enable_experimental_migration_api` in the
//! config file must flip the `ShamirDb` experimental-migration gate that
//! every wire-level `StartMigration` handler reads
//! (`handle_start_migration` → `ShamirAdminExecutor::shamir` →
//! `experimental_migration_enabled()`).
//!
//! The gate's *content* (the `experimental_feature_disabled` rejection) is
//! already covered in `shamir-db`; the TS e2e suite covers the full
//! start→status→rollback / start→commit→readable lifecycle over a real
//! socket. This file guards the narrow gap those suites leave open: that the
//! live `shamir-server` binary — which previously had NO way at all to call
//! `ShamirDb::enable_experimental_migration_api()` — actually forwards the
//! config field into that call at boot. Deleting or breaking the 3-line
//! `if config.security.enable_experimental_migration_api { ... }` block in
//! `server_launcher.rs` makes both of these tests fail.

mod common;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_server::config::Config;
use shamir_server::server::{BootstrapMode, ServerLauncher};

use common::make_test_config;

/// A server booted with `security.enable_experimental_migration_api: true`
/// must end up with the experimental-migration API enabled — i.e. a
/// subsequent `StartMigration` would clear the `experimental_feature_disabled`
/// gate instead of being rejected by it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_with_flag_enabled_turns_on_experimental_migration_api() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let temp = TempDir::new().expect("tempdir");

    let mut config: Config = make_test_config(&temp, "127.0.0.1:0");
    config.security.enable_experimental_migration_api = true;
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(b"test-password".to_vec()),
    };

    let handle = ServerLauncher { config, bootstrap }
        .launch()
        .await
        .expect("launcher boot with flag enabled");
    assert!(
        handle.experimental_migration_enabled(),
        "security.enable_experimental_migration_api: true must enable the \
         experimental migration API on the booted ShamirDb instance"
    );

    handle.shutdown().await;
}

/// A server booted with the default config (flag omitted → `false`) must
/// leave the gate disabled — preserving today's safe-by-default behavior so
/// `StartMigration` is still rejected with `experimental_feature_disabled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_with_default_config_leaves_experimental_migration_api_disabled() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let temp = TempDir::new().expect("tempdir");

    // `make_test_config` builds `security: Default::default()` — i.e. the
    // flag is never set, mirroring every existing config that predates F-15.
    let config: Config = make_test_config(&temp, "127.0.0.1:0");
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(b"test-password".to_vec()),
    };

    let handle = ServerLauncher { config, bootstrap }
        .launch()
        .await
        .expect("launcher boot with default config");
    assert!(
        !handle.experimental_migration_enabled(),
        "default config must leave the experimental migration API disabled"
    );

    handle.shutdown().await;
}
