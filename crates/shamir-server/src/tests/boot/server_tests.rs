use fs4::fs_std::FileExt;
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

use crate::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use crate::server::{BootstrapMode, ServerLauncher};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launcher_shutdown_stops_token_gc_task() {
    // Surfaces the shutdown stage timers (`log::warn!`) in the failure output.
    let _ = tracing_subscriber::fmt()
        .with_env_filter("warn")
        .with_test_writer()
        .try_init();
    let temp = tempfile::tempdir().expect("tempdir");
    let data_dir = temp.path().to_path_buf();
    let config = Config {
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        durability: Default::default(),
        transactions: Default::default(),
        tokens: Default::default(),
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: "127.0.0.1:0".into(),
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
        observability: crate::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    };
    let launcher = ServerLauncher {
        config,
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(b"test-password".to_vec()),
        },
        node_mode: crate::db_handler::NodeMode::ReadWrite,
    };
    let handle = launcher.launch().await.expect("server launch");
    let service = Arc::clone(&handle.token_service);
    assert!(!service.gc_task_finished().await);
    let shutdown = tokio::spawn(handle.shutdown());
    tokio::time::timeout(Duration::from_secs(30), async {
        while !service.gc_task_finished().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("GC stopped during shutdown");
    drop(service);
    let report = tokio::time::timeout(Duration::from_secs(30), shutdown)
        .await
        .expect("server shutdown bounded")
        .expect("shutdown task");
    assert!(report.is_clean(), "{report:?}");
}

/// Proves the advisory OS file lock mechanism used for the single-instance
/// guard:
/// 1. First `try_lock_exclusive` succeeds.
/// 2. A second `try_lock_exclusive` on the same path fails (would-block).
/// 3. After dropping the first lock, a third `try_lock_exclusive` succeeds
///    (release-on-drop).
///
/// This tests the lock primitive directly on a tempfile. The wiring from
/// `launch()` into `ServerHandle._data_dir_lock` is verified by compilation
/// (the field is populated in the `Ok(ServerHandle { ... })` constructor;
/// removing it is a compile error).
#[test]
fn data_dir_lock_blocks_second_instance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lock_path = dir.path().join(".shamir.lock");

    // 1. First lock — must succeed.
    let file1 = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open lock file");
    file1.try_lock_exclusive().expect("first lock must succeed");

    // 2. Second lock on the same path — must fail (WouldBlock).
    let file2 = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock file again");
    let err = file2
        .try_lock_exclusive()
        .expect_err("second lock must fail while first is held");
    // On Unix, fs4 returns WouldBlock; on Windows the OS error code 33
    // ("The process cannot access the file because another process has
    // locked a portion of the file") maps to Uncategorized in current
    // Rust std. Accept either.
    let is_lock_conflict =
        err.kind() == std::io::ErrorKind::WouldBlock || err.raw_os_error() == Some(33);
    assert!(
        is_lock_conflict,
        "expected WouldBlock or OS error 33, got: {err:?}",
    );

    // 3. Drop first lock → third attempt must succeed (release-on-drop).
    drop(file1);
    let file3 = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock file third time");
    file3
        .try_lock_exclusive()
        .expect("third lock must succeed after first is dropped");
}
