//! Ordered-shutdown gate: the audit stage is bounded by ONE shared deadline
//! and never lies about its outcome.
//!
//! Both tests drive the REAL production launcher over a real data dir:
//!
//! * [`shutdown_reports_audit_timeout_while_checkpoint_holds_gate`] arms the
//!   audit appender's persistence-gate hook (`test-hooks` feature) and runs a
//!   REAL checkpoint on a blocking thread inside it, so the checkpoint owns
//!   the real `parking_lot` persistence gate for as long as the test keeps
//!   it. The audit deadline must fire while that gate is still held: shutdown
//!   returns with the `audit teardown exceeded` prefix and the repository
//!   teardown still completes. The gate is released before any assertion
//!   runs, so a failing claim can never strand the checkpoint inside it.
//! * [`shutdown_report_is_unclean_when_audit_flush_fails`] arms the
//!   appender's one-shot flush fault and asserts the failed final flush
//!   reaches `ShutdownReport` exactly once (unclean, exit code 70) instead of
//!   being discarded.
//!
//! No sleeps are used for synchronization: the gate rendezvous is a condition
//! variable and every wait is bounded by [`CALL_TIMEOUT`]; an expired bound is
//! a hard failure naming its stage, never an unbounded park.

use std::future::Future;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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
    guard.mark("launched");

    // Arm the one-shot gate hook, then run a REAL checkpoint on its own
    // thread. The hook runs while that thread owns the persistence gate, so
    // blocking in it keeps the gate held for the whole shutdown stage.
    let appender = Arc::clone(&handle.audit_appender);
    // Observable for the skipped-close rule: the report says the close was not
    // attempted AND the appender was never asked to attempt one.
    let close_observable = Arc::clone(&handle.audit_appender);
    let gate = Arc::new((Mutex::new(GateState::default()), Condvar::new()));
    appender.arm_gate_acquired_hook({
        let gate = Arc::clone(&gate);
        Arc::new(move || {
            {
                let (lock, condvar) = &*gate;
                let mut state = lock.lock().expect("gate state");
                state.entered = true;
                condvar.notify_all();
            }
            wait_until(&gate, "gate release", |state| state.released);
        })
    });
    // The SCHEDULER's own checkpoint path, not a bare appender call: the
    // periodic tick runs `AuditChainWriter::checkpoint_with_appender` on a
    // blocking thread (`boot/scheduler.rs`), so holding the gate from inside
    // that call is exactly the shape a stalling scheduler tick has.
    let seam = handle
        .activity_test_seam
        .take()
        .expect("boot activity seam");
    let appender_trait: Arc<dyn AuditAppender> = handle.audit_appender.clone();
    let chain = Arc::clone(&seam.audit_chain);
    drop(seam);
    let holder = tokio::task::spawn_blocking(move || {
        chain.checkpoint_with_appender(appender_trait.as_ref())
    });
    // The checkpoint runs on a blocking thread, so waiting for the
    // rendezvous on this (current-thread runtime) task blocks nothing that
    // the handshake needs.
    wait_until(&gate, "gate rendezvous", |state| state.entered);
    guard.mark("real checkpoint holds the audit persistence gate");

    // Observations only until the gate is released: the watchdog only logs,
    // so a panic here would park the checkpoint inside the real persistence
    // gate for the rest of the process. No refusal is queued, so the drain is
    // immediate and the deadline is spent on the flusher join / final flush
    // that need the held gate.
    let shutdown = tokio::time::timeout(CALL_TIMEOUT, Box::pin(handle.shutdown())).await;
    let shutdown_returned_while_gate_held = !holder.is_finished();
    let shutdown_report = shutdown.ok();
    guard.mark("shutdown returned while the gate was still held");

    // Unconditional release, then join; assertions run only afterwards.
    release_gate(&gate);
    let checkpoint = tokio::time::timeout(CALL_TIMEOUT, holder)
        .await
        .expect("released checkpoint deadline")
        .expect("checkpoint task");
    checkpoint.expect("held checkpoint must succeed");
    guard.mark("released gate and joined the checkpoint");

    let report = shutdown_report.expect("shutdown future must finish inside the bound");
    assert!(
        shutdown_returned_while_gate_held,
        "shutdown must return while the real checkpoint still holds the gate: {report:?}"
    );
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
    // Both stoppers DID stop here (the flusher checks its stop request before
    // it ever takes the gate), so the close is submitted — and it is the close
    // itself that runs into the held gate. The observable is therefore that the
    // stage reports the close's own timeout rather than a skipped close.
    assert!(
        report
            .errors()
            .iter()
            .any(|error| error.contains("waiting for the final audit")),
        "the submitted close must report its own timeout: {report:?}"
    );
    assert_eq!(
        close_observable.final_close_attempts(),
        1,
        "with both stoppers finished the close IS submitted exactly once \
         (mutant: a close skipped while nothing is in the gate)"
    );
    assert!(
        !report
            .errors()
            .iter()
            .any(|error| error.contains("audit final close not attempted")),
        "a skipped-close report would be a lie here: {report:?}"
    );
    assert_eq!(report.exit_code(), 70, "unclean report: {report:?}");
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
    // Each distinct failure is reported once: a latch observed by more than
    // one step must not duplicate the prefix in the report.
    let flush_failures = report
        .errors()
        .iter()
        .filter(|error| error.contains("audit flush failed at shutdown"))
        .count();
    assert_eq!(
        flush_failures, 1,
        "the audit flush failure must be reported exactly once: {report:?}"
    );
    assert!(!report.is_clean(), "unclean report expected: {report:?}");
    assert_eq!(report.exit_code(), 70, "supervisor exit code");
    guard.mark("shutdown reported the audit flush failure");
}

/// The test rendezvous with the REAL persistence-gate hook.
#[derive(Clone, Copy, Default)]
struct GateState {
    /// The real checkpoint owns the persistence gate and is parked inside the
    /// hook.
    entered: bool,
    /// The test released the hook; the checkpoint may finish and drop the
    /// gate.
    released: bool,
}

/// Block until `ready` holds, bounded by [`CALL_TIMEOUT`] from a deadline
/// taken when the wait starts. An expired bound is a hard failure naming the
/// stage, so no rendezvous in this file can park a thread forever.
fn wait_until(gate: &(Mutex<GateState>, Condvar), stage: &str, ready: impl Fn(GateState) -> bool) {
    let (lock, condvar) = gate;
    let deadline = Instant::now() + CALL_TIMEOUT;
    let mut state = lock.lock().expect("gate state");
    while !ready(*state) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (next, timed_out) = condvar
            .wait_timeout(state, remaining)
            .expect("gate state wait");
        state = next;
        if timed_out.timed_out() && !ready(*state) {
            panic!("{stage} did not happen within {CALL_TIMEOUT:?}");
        }
    }
}

/// Release the parked checkpoint. Called on every path that saw the gate
/// entered, so no failure can leave the checkpoint inside it.
fn release_gate(gate: &(Mutex<GateState>, Condvar)) {
    let (lock, condvar) = gate;
    let mut state = lock.lock().expect("gate state");
    state.released = true;
    condvar.notify_all();
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
