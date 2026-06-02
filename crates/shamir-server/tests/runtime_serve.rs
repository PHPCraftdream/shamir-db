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

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    let serve_task = tokio::spawn(serve(config, bootstrap, async {
        let _ = rx.await;
    }));

    // Give the server a moment to bind listeners before triggering shutdown.
    tokio::time::sleep(Duration::from_millis(500)).await;

    tx.send(()).expect("send shutdown trigger");

    let result = tokio::time::timeout(Duration::from_secs(10), serve_task)
        .await
        .expect("serve task did not finish within 10s")
        .expect("serve task did not panic");

    assert!(result.is_ok(), "serve should return Ok(())");
}
