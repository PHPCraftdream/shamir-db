//! Ordered-shutdown gate: the audit stage is bounded by ONE shared deadline
//! and never lies about its outcome.
//!
//! Both tests drive the REAL production launcher over a real data dir:
//!
//! * [`shutdown_reports_audit_timeout_while_checkpoint_holds_gate`] arms the
//!   audit appender's persistence-gate hook (`test-hooks` feature) and runs
//!   a REAL checkpoint on a separate thread, which therefore holds the real
//!   `parking_lot` gate for as long as the test keeps it. Shutdown must
//!   report an audit timeout and must still finish the repository teardown
//!   before the gate is released — the old code awaited the flusher and then
//!   took that same gate inline, so it could never return.
//! * [`shutdown_report_is_unclean_when_audit_flush_fails`] arms the
//!   appender's one-shot flush fault and asserts the failure reaches
//!   `ShutdownReport` (unclean, exit code 70) instead of being discarded.
//!
//! No sleeps are used for synchronization: both rendezvous are channels;
//! every await is bounded by [`CALL_TIMEOUT`].

use std::future::Future;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::server::audit_chain::{AuditAppender, AuditChainWriter};
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};

use crate::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig, TlsConfig,
};
use crate::db_handler::NodeMode;
use crate::server::{BootstrapMode, ServerLauncher};

/// Generous bound: the Windows CI runner is up to 10x slower, and test (a)
/// deliberately waits out the audit deadline plus the release timeout.
const CALL_TIMEOUT: Duration = Duration::from_secs(180);

#[test]
fn shutdown_reports_audit_timeout_while_checkpoint_holds_gate() {
    on_big_stack(|| async {
        tokio::time::timeout(CALL_TIMEOUT, run_audit_timeout_while_gate_held())
            .await
            .expect("audit-timeout test bounded");
    });
}

#[test]
fn shutdown_report_is_unclean_when_audit_flush_fails() {
    on_big_stack(|| async {
        tokio::time::timeout(CALL_TIMEOUT, run_audit_flush_failure())
            .await
            .expect("audit-flush-failure test bounded");
    });
}

async fn run_audit_timeout_while_gate_held() {
    let guard = FlakeGuard::with_watchdog(
        "shutdown_reports_audit_timeout_while_checkpoint_holds_gate",
        Duration::from_secs(300),
    );
    try_install_global_tracing_ring();
    let temp = tempfile::tempdir().expect("persistent data directory");
    let handle = Box::pin(
        ServerLauncher {
            config: boot_config(temp.path().to_path_buf()),
            bootstrap: BootstrapMode::Skip,
            node_mode: NodeMode::ReadWrite,
        }
        .launch(),
    )
    .await
    .expect("production boot");
    guard.mark("launched");

    // Arm the one-shot gate hook, then run a REAL checkpoint on its own
    // thread. The hook runs while that thread owns the persistence gate, so
    // blocking in it keeps the gate held for the whole shutdown stage.
    let appender = Arc::clone(&handle.audit_appender);
    // (entered, released): the hook runs while the checkpoint owns the real
    // persistence gate, so it parks there until the test releases it.
    let gate = Arc::new((Mutex::new((false, false)), Condvar::new()));
    appender.arm_gate_acquired_hook({
        let gate = Arc::clone(&gate);
        Arc::new(move || {
            let (lock, condvar) = &*gate;
            let mut state = lock.lock().expect("gate state");
            state.0 = true;
            condvar.notify_all();
            while !state.1 {
                state = condvar.wait(state).expect("gate wait");
            }
        })
    });
    let holder = tokio::task::spawn_blocking({
        let appender = Arc::clone(&appender);
        move || appender.checkpoint(1, &[0u8; 32])
    });
    // The checkpoint runs on a blocking thread, so waiting for the
    // rendezvous on this (current-thread runtime) task blocks nothing that
    // the handshake needs.
    {
        let (lock, condvar) = &*gate;
        let mut state = lock.lock().expect("gate state");
        while !state.0 {
            state = condvar.wait(state).expect("gate wait");
        }
    }
    assert!(
        !holder.is_finished(),
        "the checkpoint must still hold the gate"
    );
    guard.mark("real checkpoint holds the audit persistence gate");

    // No refusal is queued, so the drain is immediate: the deadline is spent
    // on the flusher join / final flush that need the held gate.
    let report = tokio::time::timeout(CALL_TIMEOUT, Box::pin(handle.shutdown()))
        .await
        .expect("shutdown future must finish inside the bound");
    guard.mark("shutdown returned while the gate was still held");
    assert!(
        report
            .errors()
            .iter()
            .any(|error| error.contains("audit teardown exceeded")),
        "an audit timeout must be reported: {report:?}"
    );
    // Repository teardown ran and succeeded while the audit stage gave up:
    // the timeout bounds the audit wait, it does not skip the flush.
    assert!(
        report.repository_failures().is_empty(),
        "repository teardown must still complete: {report:?}"
    );
    assert_eq!(report.exit_code(), 70, "unclean report: {report:?}");

    {
        let (lock, condvar) = &*gate;
        let mut state = lock.lock().expect("gate state");
        state.1 = true;
        condvar.notify_all();
    }
    let _ = tokio::time::timeout(CALL_TIMEOUT, holder)
        .await
        .expect("released checkpoint deadline")
        .expect("checkpoint task");
    guard.mark("released gate and joined the checkpoint");
}

async fn run_audit_flush_failure() {
    let guard = FlakeGuard::with_watchdog(
        "shutdown_report_is_unclean_when_audit_flush_fails",
        Duration::from_secs(300),
    );
    try_install_global_tracing_ring();
    let temp = tempfile::tempdir().expect("persistent data directory");
    let mut handle = Box::pin(
        ServerLauncher {
            config: boot_config(temp.path().to_path_buf()),
            bootstrap: BootstrapMode::Skip,
            node_mode: NodeMode::ReadWrite,
        }
        .launch(),
    )
    .await
    .expect("production boot");
    let seam = handle
        .activity_test_seam
        .take()
        .expect("boot activity seam");
    let appender: Arc<dyn AuditAppender> = handle.audit_appender.clone();
    let chain = Arc::clone(&seam.audit_chain);
    drop(seam);
    // One buffered event. Arm the fault FIRST: the entry is flushed either by
    // the background flusher or by the final flush, and both paths latch, so
    // the shutdown stage always observes a flush error.
    handle.audit_appender.arm_fail_next_flush();
    let writer = AuditChainWriter::new_with_shared(chain, appender);
    writer
        .append(
            "test_shutdown_flush_failure",
            "tcp",
            "",
            "",
            [0u8; 8],
            "ok",
            Vec::new(),
            UnixNanos::try_now().expect("audit timestamp").as_u64(),
        )
        .expect("buffered audit event");
    guard.mark("buffered one event with the flush fault armed");

    let report = tokio::time::timeout(CALL_TIMEOUT, Box::pin(handle.shutdown()))
        .await
        .expect("shutdown deadline");
    assert!(
        report
            .errors()
            .iter()
            .any(|error| error.contains("audit flush failed at shutdown")),
        "the audit flush error must reach the report: {report:?}"
    );
    assert!(!report.is_clean(), "unclean report expected: {report:?}");
    assert_eq!(report.exit_code(), 70, "supervisor exit code");
    guard.mark("shutdown reported the audit flush failure");
}

fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(body())
        })
        .expect("spawn test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

/// Minimal production launcher config over a real temp data dir: one plain
/// loopback HTTP listener and observability on port 0 (an `Option` seam is
/// only captured when observability is enabled).
fn boot_config(data_dir: std::path::PathBuf) -> Config {
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
