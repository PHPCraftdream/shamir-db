//! Ordered-shutdown gate: the store-release barrier's outcome is REAL, not a
//! log line, and it reaches the shutdown report.
//!
//! [`a_held_store_lock_keeps_the_release_barrier_failing`] pins the source of
//! that report entry: a fjall store root whose `lock` file is held by another
//! handle makes [`wait_stores_released`] return the still-locked root when the
//! budget expires, and `Ok` once the lock is released. Mutant: a barrier that
//! reports `Ok` (or an empty list) on a held lock, or one that never stops
//! waiting, makes the first or the second half of the test fail.
//!
//! [`a_store_that_outlives_the_teardown_reaches_the_shutdown_report`] drives
//! the REAL launcher: a stale store root created before boot is held for the
//! whole shutdown, so the release barrier times out and the entry must appear
//! in `ShutdownReport::errors()`. Mutant: the former behaviour, which only
//! logged a `tracing::warn!` and left the report clean — the assertion then
//! fails with a clean report and exit code 0.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use shamir_private_dir::create_private_dir;
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};

use crate::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig, TlsConfig,
};
use crate::db_handler::NodeMode;
use crate::server::wait_stores_released;
use crate::server::{BootstrapMode, ServerLauncher};

/// Generous bound: the Windows CI runner is up to 10x slower than a
/// workstation, and the launcher test deliberately waits out the barrier's own
/// release timeout.
const BOUND: Duration = Duration::from_secs(180);

#[test]
fn a_held_store_lock_keeps_the_release_barrier_failing() {
    on_big_stack(|| async {
        tokio::time::timeout(BOUND, run_held_store_lock())
            .await
            .expect("store-release barrier test bounded");
    });
}

#[test]
fn a_store_that_outlives_the_teardown_reaches_the_shutdown_report() {
    let guard = FlakeGuard::with_watchdog(
        "a_store_that_outlives_the_teardown_reaches_the_shutdown_report",
        Duration::from_secs(300),
    );
    try_install_global_tracing_ring();
    let temp = tempfile::tempdir().expect("test root");
    let data_dir = temp.path().join("data");
    create_private_dir(&data_dir).expect("private data directory");
    // A stale store root (fjall marks one by `lock` + `version`) with its lock
    // HELD for the whole shutdown: boot ignores the unknown directory, the
    // release barrier finds it and times out.
    let stale = data_dir.join("stale_store");
    std::fs::create_dir_all(&stale).expect("stale store dir");
    std::fs::write(stale.join("version"), b"v").expect("version marker");
    std::fs::write(stale.join("lock"), b"").expect("lock file");
    let held = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(stale.join("lock"))
        .expect("open the stale store lock");
    held.try_lock()
        .expect("the test holds the stale store lock");
    guard.mark("stale store lock held before boot");

    let handle = run_blocking(move || {
        let config = boot_config(data_dir.clone());
        block_on_launch(config)
    });
    guard.mark("launcher booted");
    let report = run_blocking(move || block_on_shutdown(handle, Duration::from_secs(30)));
    guard.mark("shutdown drained with the store still locked");

    // Unconditional release, then assert: a failing claim never leaves the
    // lock behind for the next test.
    assert!(held.unlock().is_ok(), "release the stale store lock");
    let still_reported = report
        .errors()
        .iter()
        .any(|error| error.contains("stores still held after the release timeout"));
    assert!(
        still_reported,
        "a store that outlives the teardown must reach the shutdown report, not only a \
         `tracing::warn!` (mutant: the barrier's failure is logged and the report stays \
         clean): {report:?}"
    );
    assert!(
        !report.is_clean(),
        "a store that outlived the teardown makes the shutdown unclean: {report:?}"
    );
}

/// Hold one fjall-shaped store root's `lock` and observe the barrier from both
/// sides of the release.
async fn run_held_store_lock() {
    let temp = tempfile::tempdir().expect("test root");
    let store = temp.path().join("counters");
    std::fs::create_dir_all(&store).expect("store dir");
    std::fs::write(store.join("version"), b"v").expect("version marker");
    std::fs::write(store.join("lock"), b"").expect("lock file");
    let held = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.join("lock"))
        .expect("open store lock");
    held.try_lock().expect("the test holds the store lock");

    let locked = wait_stores_released(temp.path(), Duration::from_millis(100)).await;
    let still = locked.expect_err(
        "a held store lock must fail the barrier: an `Ok` here would report a released data \
         directory while a writer still owns it",
    );
    assert_eq!(
        still,
        vec![store.clone()],
        "the barrier names the store root it is still waiting for"
    );

    held.unlock().expect("release the store lock");
    wait_stores_released(temp.path(), BOUND)
        .await
        .expect("a released store lock must satisfy the barrier");
    drop(held);
}

/// A server config with no fixed ports: every listener and the observability
/// endpoint bind `127.0.0.1:0`, so parallel tests never share `7331-7333`.
fn boot_config(data_dir: PathBuf) -> Config {
    Config {
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
        argon2_concurrent_max: 64,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Http,
            addr: "127.0.0.1:0".into(),
            profile: None,
            http: crate::boot::config::HttpListenerSettings {
                tls: Some(false),
                ..Default::default()
            },
            path: None,
            kdf_override: None,
            browser_origin_allowlist: Vec::new(),
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: crate::config::SecurityConfig::default(),
        audit: Default::default(),
        observability: ObservabilityConfig {
            addr: "127.0.0.1:0".into(),
            allow_public_metrics: false,
        },
        replication: None,
        backup: None,
    }
}

fn block_on_launch(config: Config) -> crate::server::ServerHandle {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let launched = runtime.block_on(async move {
        tokio::time::timeout(
            BOUND,
            Box::pin(
                ServerLauncher {
                    config,
                    bootstrap: BootstrapMode::Skip,
                    node_mode: NodeMode::ReadWrite,
                }
                .launch(),
            ),
        )
        .await
    });
    match launched {
        Ok(Ok(handle)) => handle,
        Ok(Err(error)) => panic!("launcher boot failed: {error}"),
        Err(_) => panic!("launcher boot did not finish within {BOUND:?}"),
    }
}

fn block_on_shutdown(
    handle: crate::server::ServerHandle,
    deadline: Duration,
) -> shamir_db::ShutdownReport {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    runtime.block_on(async move {
        tokio::time::timeout(BOUND, crate::runtime::drain_server(handle, deadline))
            .await
            .expect("the embedding shutdown must finish within the bound")
    })
}

fn run_blocking<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    run_blocking(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(body())
    })
}
