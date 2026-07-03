//! Integration test for the `serve` runtime core (RM-1 / RM-2).
//!
//! Proves that `serve(config, bootstrap, shutdown_future)` boots the server
//! and shuts down gracefully when the arbitrary shutdown future resolves.
//! Real OS signals are NOT tested here — the seam is the shutdown future
//! itself, which every runtime mode reuses.

use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::runtime::serve;
use shamir_server::server::BootstrapMode;

fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn make_test_config(temp: &TempDir) -> Config {
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
            addr: "127.0.0.1:0".to_string(),
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
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
        },
        replication: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serve_boots_and_shuts_down_on_trigger() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(b"test-password".to_vec()),
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    // init_done: server fires this AFTER listeners bind + bootstrap completes.
    // Previously the test used a fixed `sleep(500ms)` here — under load
    // (parallel nextest run, Argon2 KDF + self-signed cert gen + bind) this
    // was nondeterministic: shutdown fired before the server was ready,
    // leading to spurious failures. Synchronise on the actual readiness
    // signal — no time-based race remains.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
    let mut ready_tx = Some(ready_tx);

    let serve_task = tokio::spawn(serve(
        config,
        bootstrap,
        async {
            let _ = shutdown_rx.await;
        },
        move || {
            // Called by serve() once listeners are bound + bootstrap is done.
            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(());
            }
        },
    ));

    // Wait for the server's actual readiness signal (no fixed sleep).
    // Generous timeout: cold-start Argon2 + cert gen can take seconds on
    // a contended CI / dev box. 30 s is well above the legit envelope and
    // still catches genuine bring-up regressions.
    tokio::time::timeout(Duration::from_secs(30), ready_rx)
        .await
        .expect("server did not reach ready state within 30s")
        .expect("ready signal channel closed (server panicked during init)");

    shutdown_tx.send(()).expect("send shutdown trigger");

    let result = tokio::time::timeout(Duration::from_secs(10), serve_task)
        .await
        .expect("serve task did not finish within 10s of shutdown")
        .expect("serve task did not panic");

    assert!(result.is_ok(), "serve should return Ok(())");
}
