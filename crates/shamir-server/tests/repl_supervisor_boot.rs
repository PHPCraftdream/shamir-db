//! 386-c — the follower-replication [`SubscriptionSupervisor`] is wired into
//! the server boot path. These tests prove the wiring does not break boot:
//! the supervisor is constructed, `reconcile()` runs after the listeners are
//! bound (a no-op for an empty `system/subscriptions` catalogue), the periodic
//! reconcile-tick task is spawned, and `shutdown()` cancels it cleanly.
//!
//! The production `WireReplSource` path (real TLS+SCRAM upstream) needs a
//! second live server and is exercised by the two-server e2e (#388); here we
//! only assert boot + reconcile + shutdown are non-breaking with the default
//! `replication = None` (no replicator credentials → no-op factory).

mod common;

use std::time::Duration;

use tempfile::TempDir;

use shamir_server::config::ReplicationConfig;

/// Boot with the default `replication = None`, then shut down. The supervisor
/// is constructed and reconciled with an empty catalogue; shutdown must be
/// clean (the reconcile-tick task is cancelled and joined).
#[tokio::test]
async fn boot_and_shutdown_with_no_replication_config() {
    let temp = TempDir::new().expect("temp dir");
    let handle = common::spawn_ephemeral(&temp, b"admin-password-123").await;
    assert!(
        handle.first_tls_exporter_addr().is_some(),
        "server bound a TCP+TLS listener",
    );
    // Give the reconcile-tick task a moment to be scheduled; it must not panic
    // or wedge the runtime with an empty catalogue.
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.shutdown().await;
}

/// Boot with a populated `[replication]` section (node_id + replicator creds).
/// No upstream exists, so any (absent) subscription cannot progress, but the
/// prod `WireReplSource` factory path is constructed and boot + shutdown stay
/// clean. Proves the credentialled branch of the boot wiring compiles and runs.
#[tokio::test]
async fn boot_and_shutdown_with_replication_config() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let temp = TempDir::new().expect("temp dir");
    let mut config = common::make_test_config(&temp, "127.0.0.1:0");
    config.replication = Some(ReplicationConfig {
        node_id: Some("follower-test".into()),
        replicator_user: Some("replicator".into()),
        replicator_password: Some("repl-secret".into()),
        server_name: "localhost".into(),
    });

    let bootstrap = shamir_server::server::BootstrapMode::Password {
        username: "admin".into(),
        password: zeroize::Zeroizing::new(b"admin-password-123".to_vec()),
    };
    let handle = shamir_server::server::ServerLauncher { config, bootstrap }
        .launch()
        .await
        .expect("launcher boot");

    assert!(handle.first_tls_exporter_addr().is_some());
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.shutdown().await;
}
