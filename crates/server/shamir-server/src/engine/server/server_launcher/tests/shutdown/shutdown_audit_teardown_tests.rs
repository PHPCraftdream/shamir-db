//! Ordered-shutdown gate: the audit stage is bounded by ONE shared deadline
//! and never lies about its outcome.
//!
//! The first two tests drive the REAL production launcher over a real data
//! dir:
//!
//! * [`shutdown_reports_audit_timeout_while_checkpoint_holds_gate`] arms the
//!   audit appender's persistence-gate hook (`test-hooks` feature) and runs a
//!   REAL checkpoint on a blocking thread inside it, so the checkpoint owns
//!   the real `parking_lot` persistence gate for as long as the test keeps
//!   it. The audit deadline must fire while that gate is still held: shutdown
//!   returns with the `audit teardown exceeded` prefix exactly ONCE and the
//!   repository teardown still completes. The gate is released before any
//!   assertion runs, so a failing claim can never strand the checkpoint
//!   inside it.
//!
//!   Post-close contract this window pins: a checkpoint that was admitted AND
//!   already passed the DECISIVE in-gate check
//!   (`refuse_closed_checkpoint_under_gate`) before the stop finishes `Ok`
//!   and writes its anchor, while a checkpoint that only reaches the gate
//!   after the stop is refused with the typed post-close refusal and writes
//!   nothing. The persistence-gate hook fires inside
//!   `acquire_persistence_gate`, i.e. BEFORE that decisive check, so this test
//!   parks in the second case and pins the REFUSAL half: the resumed
//!   checkpoint comes back as
//!   `AuditError::Persistence("... refusing a post-close checkpoint")`, the
//!   fence counts it exactly once (the refusal returns before
//!   `checkpoint_locked`, the only anchor writer on that path) and the report
//!   still names the audit timeout exactly once. The "finishes `Ok`" half is
//!   NOT covered by this test: no test hook exists between the decisive check
//!   and `checkpoint_locked`.
//! * [`shutdown_report_is_unclean_when_audit_flush_fails`] arms the
//!   appender's one-shot flush fault and asserts the failed final flush
//!   reaches `ShutdownReport` exactly once (unclean, exit code 70) instead of
//!   being discarded.
//!
//! No sleeps are used for synchronization: every rendezvous is a condition
//! variable, a `Notify` permit or a bounded channel, and every wait is bounded
//! by [`CALL_TIMEOUT`] (or the stage's own budget); an expired bound is a hard
//! failure naming its stage, never an unbounded park.
//!
//! The remaining tests pin the individual teardown rules against mutants:
//!
//! * [`audit_teardown_skips_final_close_when_scheduler_tick_never_returns`] —
//!   a scheduler-owned tick parked INSIDE the persistence gate keeps
//!   `Scheduler::join` pending, so the final close must not be submitted.
//! * [`panicking_flusher_reports_one_latched_failure`] — one latched flusher
//!   failure reaches the report exactly once, whatever prefix the reader adds.
//! * [`final_close_is_skipped_when_the_refusal_drain_does_not_settle`] — the
//!   close is skipped for the refusal drain too, not only for a stalled
//!   scheduler or flusher.
//! * [`final_close_runs_while_the_blocking_pool_is_saturated`] — the close
//!   runs on its own thread, never as a shared-pool ticket.

use std::future::Future;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use shamir_connect::common::time::UnixNanos;
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditError,
};
use shamir_connect::server::lockout::InMemoryLockoutStore;
use shamir_connect::server::rate_limit::InMemoryRateLimiter;
use shamir_connect::server::resume::InMemoryConsumedCounters;
use shamir_connect::server::rotation::ServerIdentityState;
use shamir_connect::server::session::SessionStore;
use shamir_db::ShutdownReport;
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};
use tokio::sync::Notify;

use crate::audit_appender::FjallAuditAppender;
use crate::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig, TlsConfig,
};
use crate::db_handler::{CancelRefusalAudit, NodeMode};
use crate::scheduler::{Scheduler, SchedulerConfig, SchedulerInputs};
use crate::server::server_launcher::audit_teardown::{teardown_audit, AuditTeardown};
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

    // The ordinary background flusher can acquire the gate before the
    // checkpoint. The hook therefore re-arms itself for any non-holder
    // acquisition and parks only the actual checkpoint thread.
    let appender = Arc::clone(&handle.audit_appender);
    // Observe either teardown outcome: the flusher may already have work in
    // flight, or may stop while idle and allow the final close to be submitted.
    let close_observable = Arc::clone(&handle.audit_appender);
    let gate = Arc::new((Mutex::new(GateState::default()), Condvar::new()));
    let holder_thread = Arc::new(OnceLock::new());
    arm_checkpoint_holder_gate_hook(
        Arc::clone(&appender),
        Arc::clone(&holder_thread),
        Arc::clone(&gate),
    );
    // Unconditional release on EVERY exit path (return, `?` or panic): the
    // parked hook's own wait is bounded, and this guard frees it immediately
    // instead of leaving the suite to wait out [`CALL_TIMEOUT`]. The explicit
    // release below still runs first on the happy path; releasing twice is
    // idempotent.
    let _release_on_drop = GateRelease(&gate);
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
        let _ = holder_thread.set(std::thread::current().id());
        chain.checkpoint_with_appender(appender_trait.as_ref())
    });
    // The checkpoint runs on a blocking thread, so waiting for the
    // rendezvous on this (current-thread runtime) task blocks nothing that
    // the handshake needs.
    wait_until(&gate, "gate rendezvous", |state| state.entered);
    guard.mark("real checkpoint holds the audit persistence gate");

    // Observe shutdown while the manual checkpoint holds the gate. The
    // scheduled audit scheduler is stopped and joined separately; the flusher
    // may be in flight or idle, so its report marker is observed after release.
    let shutdown = tokio::time::timeout(CALL_TIMEOUT, Box::pin(handle.shutdown())).await;
    let shutdown_returned_while_gate_held = !holder.is_finished();
    let shutdown_report = shutdown.ok();
    guard.mark("shutdown returned while the gate was still held");

    // Unconditional release, then join; assertions run only afterwards.
    release_gate(&gate);
    let refusal = tokio::time::timeout(CALL_TIMEOUT, holder)
        .await
        .expect("released checkpoint deadline")
        .expect("checkpoint task")
        .expect_err(
            "the resumed checkpoint only reached the persistence gate AFTER the close set \
             the shutdown flag, so the DECISIVE in-gate recheck must refuse it instead of \
             writing the anchor",
        );
    guard.mark("released gate and joined the checkpoint");

    // The checkpoint parked inside `acquire_persistence_gate`, before the
    // decisive `refuse_closed_checkpoint_under_gate` check. Because this test
    // cannot access the appender's persisted-anchor reader, the stronger
    // rejection-before-`checkpoint_locked` evidence below proves the resumed
    // checkpoint cannot change the persisted anchor row.
    assert!(
        matches!(&refusal, AuditError::Persistence(_)),
        "the resumed checkpoint must fail as a persistence refusal, not {refusal:?}"
    );
    let refusal_text = refusal.to_string();
    assert!(
        refusal_text.contains("refusing a post-close checkpoint"),
        "the refusal must be the typed post-close fence, not any other failure: {refusal_text}"
    );
    // The decisive half returns BEFORE `checkpoint_locked`, the only writer
    // of the anchor row on this path, so the persisted anchor remains unchanged.
    assert_eq!(
        appender.refused_checkpoint_attempts(),
        1,
        "the in-gate recheck must refuse the resumed checkpoint exactly once and before \
         `checkpoint_locked`, so the resumed checkpoint writes no anchor row"
    );

    let report = shutdown_report.expect("shutdown future must finish inside the bound");
    assert!(
        shutdown_returned_while_gate_held,
        "shutdown must return while the real checkpoint still holds the gate: {report:?}"
    );
    // Exactly ONE audit timeout: the held gate makes the submitted final
    // close the only thing that expires in this stage, and the shutdown report
    // must name it once — not once per observer.
    let audit_timeouts = report
        .errors()
        .iter()
        .filter(|error| error.contains("audit teardown exceeded"))
        .count();
    assert_eq!(
        audit_timeouts, 1,
        "exactly one audit timeout must be reported: {report:?}"
    );
    // Repository teardown ran and succeeded while the audit stage gave up:
    // the timeout bounds the audit wait, it does not skip the flush.
    assert!(
        report.repository_failures().is_empty(),
        "repository teardown must still complete: {report:?}"
    );
    // The test parks the manual checkpoint, not a scheduler tick. The flusher
    // may already have submitted a blocking job (then its join times out and
    // the close is skipped), or it may observe stop while idle (then the final
    // close is submitted and times out on this gate). Both dispositions are
    // truthful outcomes; distinguish them from the report rather than claiming
    // the holder alone proves a flusher job is in flight.
    let final_close_skipped = report
        .errors()
        .iter()
        .find(|error| error.contains("audit final close not attempted"));
    let flusher_timeout = report
        .errors()
        .iter()
        .any(|error| error.contains("waiting for the audit flusher"));
    let final_close_timeout = report
        .errors()
        .iter()
        .any(|error| error.contains("waiting for the final audit flush"));
    let teardown_wait_timeouts = report
        .errors()
        .iter()
        .filter(|error| {
            error.contains("waiting for the audit flusher")
                || error.contains("waiting for the final audit flush")
        })
        .count();
    assert_eq!(
        teardown_wait_timeouts, 1,
        "exactly one teardown wait must time out: {report:?}"
    );
    assert_ne!(
        flusher_timeout, final_close_timeout,
        "the one wait timeout must identify exactly one teardown phase: {report:?}"
    );
    assert_eq!(
        final_close_skipped.is_some(),
        flusher_timeout,
        "the close is skipped iff the flusher failed to stop within the deadline: {report:?}"
    );
    assert_eq!(
        close_observable.final_close_attempts(),
        if final_close_timeout { 1 } else { 0 },
        "a final close attempt must match the observed final-close timeout"
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

/// A real [`Scheduler`] owns the audit checkpoint tick. One tick parked inside
/// the persistence gate keeps `Scheduler::join` from finishing, so the audit
/// stage must report the scheduler timeout and submit NO final close.
///
/// Mutant killed: a teardown that submits the final close even though the
/// scheduler had not stopped.
#[test]
fn audit_teardown_skips_final_close_when_scheduler_tick_never_returns() {
    on_big_stack(|| async {
        tokio::time::timeout(CALL_TIMEOUT, run_scheduler_tick_never_returns())
            .await
            .expect("scheduler-tick teardown test bounded");
    });
}

async fn run_scheduler_tick_never_returns() {
    let guard = FlakeGuard::with_watchdog(
        "audit_teardown_skips_final_close_when_scheduler_tick_never_returns",
        Duration::from_secs(300),
    );
    try_install_global_tracing_ring();
    let temp = tempfile::tempdir().expect("audit teardown directory");
    // STRICT appender: it has no background flusher, so the ONE gate
    // acquisition this test can observe is the scheduler's own checkpoint
    // tick — a batched flusher's periodic job can never steal the hook.
    let appender = FjallAuditAppender::open_strict(temp.path()).expect("strict audit appender");

    let scheduler = Scheduler::spawn(
        SchedulerInputs {
            counters: Arc::new(InMemoryConsumedCounters::new()),
            lockout: Arc::new(InMemoryLockoutStore::new()),
            rate_limit: Arc::new(InMemoryRateLimiter::new(0)),
            session_store: Arc::new(SessionStore::new()),
            session_max_age_ns: 24 * 60 * 60 * 1_000_000_000,
            session_idle_ttl_ns: 30 * 60 * 1_000_000_000,
            audit_chain: Arc::new(AuditChain::new([7u8; 32])),
            audit_appender: appender.clone(),
            identity: Arc::new(ServerIdentityState::fresh()),
        },
        SchedulerConfig {
            counter_gc_period: Duration::from_secs(3600),
            lockout_gc_period: Duration::from_secs(3600),
            rate_limit_gc_period: Duration::from_secs(3600),
            session_gc_period: Duration::from_secs(3600),
            // The production period is 60 s, which no test can reach inside
            // its bound: only the audit tick is shortened, everything else is
            // parked far beyond the test.
            audit_checkpoint_period: Duration::from_millis(10),
            identity_finalize_period: Duration::from_secs(3600),
        },
    );

    // The tick runs `AuditChainWriter::checkpoint_with_appender` on a blocking
    // thread (`boot/scheduler.rs`), i.e. the exact production checkpoint path.
    let gate = Arc::new((Mutex::new(GateState::default()), Condvar::new()));
    let entered = Arc::new(Notify::new());
    appender.arm_gate_acquired_hook({
        let gate = Arc::clone(&gate);
        let entered = Arc::clone(&entered);
        Arc::new(move || {
            {
                let (lock, condvar) = &*gate;
                let mut state = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.entered = true;
                condvar.notify_all();
            }
            entered.notify_one();
            wait_until(&gate, "gate release", |state| state.released);
        })
    });
    tokio::time::timeout(CALL_TIMEOUT, entered.notified())
        .await
        .expect("the scheduler-owned checkpoint tick must reach the persistence gate");
    guard.mark("scheduler-owned tick holds the persistence gate");

    let queue = Arc::new(CancelRefusalAudit::default());
    let mut report = ShutdownReport::default();
    let completed = teardown_audit(
        AuditTeardown {
            cancel_refusal_audit: &queue,
            appender: &appender,
            scheduler,
            #[cfg(test)]
            drain_probe: None,
        },
        &mut report,
    )
    .await;
    // The tick is still parked: unconditional release before any assertion so
    // a failing claim can never strand a thread inside the gate.
    release_gate(&gate);
    guard.mark("teardown returned with the scheduler tick still parked");

    assert!(
        !completed,
        "a timed-out scheduler join is not a completed stage: {report:?}"
    );
    assert!(
        report
            .errors()
            .iter()
            .any(|error| error.contains("scheduler did not stop within")),
        "the scheduler join timeout must be reported: {report:?}"
    );
    let skipped = report
        .errors()
        .iter()
        .find(|error| error.contains("audit final close not attempted"))
        .unwrap_or_else(|| panic!("the final close must be skipped: {report:?}"));
    assert!(
        skipped.contains("refusal-audit drain"),
        "the skipped close must name the scheduler/flusher/refusal-drain trio: {skipped}"
    );
    assert_eq!(
        appender.final_close_attempts(),
        0,
        "no final close may be submitted while the scheduler tick is still inside the gate"
    );
    assert_eq!(report.exit_code(), 70, "unclean report: {report:?}");
}

/// A background flusher job that PANICS after taking the buffer latches the
/// failure; the join reports it and the close re-reads the SAME latch under
/// the `io: ` prefix. One failure must reach the report exactly ONCE whatever
/// prefix the reader adds.
///
/// Mutant killed: comparing the raw formatted strings instead of the latched
/// cause (the report then carries TWO entries for one failure).
#[test]
fn panicking_flusher_reports_one_latched_failure() {
    on_big_stack(|| async {
        tokio::time::timeout(CALL_TIMEOUT, run_panicking_flusher())
            .await
            .expect("panicking-flusher test bounded");
    });
}

async fn run_panicking_flusher() {
    let guard = FlakeGuard::with_watchdog(
        "panicking_flusher_reports_one_latched_failure",
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
    let close_observable = Arc::clone(&handle.audit_appender);
    let appender: Arc<dyn AuditAppender> = handle.audit_appender.clone();
    let seam = handle
        .activity_test_seam
        .take()
        .expect("boot activity seam");
    let chain = Arc::clone(&seam.audit_chain);
    drop(seam);

    // The gate hook runs on the flusher's blocking thread inside
    // `flush_buffer_locked`'s gate: panicking there becomes the job panic the
    // flusher latches as `audit background flush job failed: ...`.
    let entered = Arc::new(Notify::new());
    let writer = AuditChainWriter::new_with_shared(chain, appender);
    writer
        .append(
            "test_panicking_flusher",
            "tcp",
            "",
            "",
            [0u8; 8],
            "ok",
            Vec::new(),
            UnixNanos::try_now().expect("audit timestamp").as_u64(),
        )
        .expect("buffered audit event");
    // Arm AFTER the append: an earlier job panic would latch the appender and
    // refuse the append itself.
    handle.audit_appender.arm_gate_acquired_hook({
        let entered = Arc::clone(&entered);
        Arc::new(move || {
            entered.notify_one();
            panic!("test: audit flusher blocking job panic");
        })
    });
    // The periodic flush job is the only gate acquirer here (the production
    // scheduler tick is 60 s away), so this rendezvous proves the job has
    // started; from here its panic, the latch and the join error are ordered.
    tokio::time::timeout(CALL_TIMEOUT, entered.notified())
        .await
        .expect("the background flush job must reach the persistence gate");
    guard.mark("flusher job took the gate and panicked");

    let report = tokio::time::timeout(CALL_TIMEOUT, Box::pin(handle.shutdown()))
        .await
        .expect("shutdown deadline");
    guard.mark("shutdown returned after the panicking flusher");

    let cause = "audit background flush job failed";
    let latch_entries: Vec<&String> = report
        .errors()
        .iter()
        .filter(|error| error.contains(cause))
        .collect();
    assert_eq!(
        latch_entries.len(),
        1,
        "one failure, one report entry whatever prefix the reader adds: {report:?}"
    );
    assert!(
        latch_entries[0].starts_with("audit flusher task did not join cleanly"),
        "the join, not the final close, must be the reporting step: {}",
        latch_entries[0]
    );
    assert!(
        !report
            .errors()
            .iter()
            .any(|error| error.contains("audit flush failed at shutdown") && error.contains(cause)),
        "the same latch must not be reported again under the flush prefix: {report:?}"
    );
    assert_eq!(
        close_observable.final_close_attempts(),
        1,
        "a job that already ended still allows the final close"
    );
    assert!(!report.is_clean(), "unclean report expected: {report:?}");
    assert_eq!(report.exit_code(), 70, "supervisor exit code");
}

#[path = "audit_flusher/mod.rs"]
mod audit_flusher_tests;

/// A final close is skipped when the refusal-audit drain itself did not
/// settle — even though the scheduler and the flusher both stopped — because
/// an admitted refusal append may still reach its own inline checkpoint.
///
/// Mutant killed: the round-2 rule `!(scheduler_stopped && flusher_stopped)`
/// that submits the close although the drain could not be established.
#[test]
fn final_close_is_skipped_when_the_refusal_drain_does_not_settle() {
    on_big_stack(|| async {
        tokio::time::timeout(CALL_TIMEOUT, run_refusal_drain_expires())
            .await
            .expect("refusal-drain teardown test bounded");
    });
}

async fn run_refusal_drain_expires() {
    let guard = FlakeGuard::with_watchdog(
        "final_close_is_skipped_when_the_refusal_drain_does_not_settle",
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
    let close_observable = Arc::clone(&handle.audit_appender);
    // The test seam holds the handler (and its stores); drop it so the
    // release barrier can see every store go.
    drop(handle.activity_test_seam.take());
    // One admitted refusal append that never finishes: the held admission
    // slot keeps the queue non-idle, so the drain spends its own budget and
    // teardown's `drain_settled` stays false.
    let held = handle.cancel_refusal_audit.hold_admission_slot_for_test();
    let (pending, _pending_rx) = tokio::sync::oneshot::channel();
    handle.refusal_drain_pending = Some(pending);

    let report = tokio::time::timeout(CALL_TIMEOUT, Box::pin(handle.shutdown()))
        .await
        .expect("shutdown deadline");
    drop(held);
    guard.mark("shutdown returned after the refusal drain expired");

    assert!(
        report
            .errors()
            .iter()
            .any(|error| error.contains("refusal audit drain expired")),
        "the expired refusal drain must be reported: {report:?}"
    );
    let skipped = report
        .errors()
        .iter()
        .find(|error| error.contains("audit final close not attempted"))
        .unwrap_or_else(|| panic!("the final close must be skipped: {report:?}"));
    assert!(
        skipped.contains("refusal-audit drain"),
        "the skipped close must say the refusal-audit drain did not settle: {skipped}"
    );
    // The skip must come from the DRAIN: neither other stopper timed out.
    assert!(
        !report
            .errors()
            .iter()
            .any(|error| error.contains("scheduler did not stop within")),
        "the scheduler stopped, so only the drain can explain the skip: {report:?}"
    );
    assert!(
        !report
            .errors()
            .iter()
            .any(|error| error.contains("waiting for the audit flusher")),
        "the flusher stopped, so only the drain can explain the skip: {report:?}"
    );
    assert_eq!(
        close_observable.final_close_attempts(),
        0,
        "an admitted refusal append may still checkpoint, so no close may be submitted"
    );
    assert_eq!(report.exit_code(), 70, "supervisor exit code");
}

/// The final close runs on its OWN dedicated thread, never as a ticket in the
/// shared blocking pool: with that pool fully occupied it still enters the
/// flush body and completes.
///
/// Mutant killed: submitting the final close through `spawn_blocking` on the
/// shared pool (with the pool held, that submission never runs, the close
/// count stays 0 and the stage times out).
#[test]
fn final_close_runs_while_the_blocking_pool_is_saturated() {
    on_big_stack_with_blocking_threads(1, || async {
        tokio::time::timeout(CALL_TIMEOUT, run_final_close_with_saturated_pool())
            .await
            .expect("saturated-pool close test bounded");
    });
}

async fn run_final_close_with_saturated_pool() {
    let guard = FlakeGuard::with_watchdog(
        "final_close_runs_while_the_blocking_pool_is_saturated",
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
    // The test seam holds the handler (and its stores); drop it so the
    // release barrier can see every store go.
    drop(handle.activity_test_seam.take());
    let close_observable = Arc::clone(&handle.audit_appender);
    // Nothing is buffered, and the flusher is stopped AND joined BEFORE the
    // pool is held: only the final close can take the one-shot gate hook.
    handle.audit_appender.request_flusher_stop();
    tokio::time::timeout(CALL_TIMEOUT, handle.audit_appender.join_flusher())
        .await
        .expect("flusher stop deadline")
        .expect("flusher stopped cleanly");
    guard.mark("flusher stopped before the pool was held");

    // This runtime has ONE blocking thread; the task below IS that thread.
    let pool = Arc::new((Mutex::new(GateState::default()), Condvar::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let occupant = tokio::task::spawn_blocking({
        let pool = Arc::clone(&pool);
        move || {
            let _ = started_tx.send(());
            wait_until(&pool, "blocking pool release", |state| state.released);
        }
    });
    tokio::time::timeout(CALL_TIMEOUT, started_rx)
        .await
        .expect("the only blocking-pool worker must start the occupying job")
        .expect("occupant started");
    guard.mark("the blocking pool is fully occupied");

    let gate = Arc::new((Mutex::new(GateState::default()), Condvar::new()));
    let entered = Arc::new(Notify::new());
    handle.audit_appender.arm_gate_acquired_hook({
        let gate = Arc::clone(&gate);
        let entered = Arc::clone(&entered);
        Arc::new(move || {
            {
                let (lock, condvar) = &*gate;
                let mut state = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.entered = true;
                condvar.notify_all();
            }
            entered.notify_one();
            wait_until(&gate, "close release", |state| state.released);
        })
    });

    let mut shutdown = Box::pin(handle.shutdown());
    // Observations only until BOTH gates are released: a panic here would
    // strand the pool occupant and the close thread, so the assertions run
    // after the unconditional release below.
    let outcome = tokio::time::timeout(CALL_TIMEOUT, async {
        tokio::select! {
            _ = entered.notified() => None,
            report = shutdown.as_mut() => Some(report),
        }
    })
    .await;
    let reached_gate = if let Ok(slot) = &outcome {
        slot.is_none()
    } else {
        false
    };
    let close_ran_while_pool_held = reached_gate && !occupant.is_finished();
    guard.mark("final close inside the flush body with the blocking pool held");
    release_gate(&gate);
    release_gate(&pool);
    let report = match outcome {
        Ok(Some(report)) => report,
        Ok(None) => tokio::time::timeout(CALL_TIMEOUT, shutdown)
            .await
            .expect("shutdown deadline"),
        Err(_) => {
            panic!("the final close never reached the persistence gate within {CALL_TIMEOUT:?}")
        }
    };
    guard.mark("shutdown returned after the pool was released");

    assert!(
        reached_gate,
        "the close must reach the gate before shutdown returns: {report:?}"
    );
    assert!(
        close_ran_while_pool_held,
        "the close must run while every blocking-pool thread is parked"
    );
    assert_eq!(
        close_observable.final_close_attempts(),
        1,
        "the dedicated close thread must still submit exactly one close"
    );
    assert!(
        !report
            .errors()
            .iter()
            .any(|error| error.contains("audit flush task did not complete")),
        "a close that could not run must not be reported: {report:?}"
    );
    assert!(
        !report
            .errors()
            .iter()
            .any(|error| error.contains("waiting for the final audit")),
        "the close must not time out on the saturated pool: {report:?}"
    );
}

/// Arm a one-shot gate hook that only parks the designated checkpoint-holder
/// thread. Other gate acquisitions (notably the background flusher) re-arm the
/// hook; `audit_appender.rs` removes the hook and releases its slot lock before
/// invoking it, so re-arming here is safe.
fn arm_checkpoint_holder_gate_hook(
    appender: Arc<FjallAuditAppender>,
    holder_thread: Arc<OnceLock<std::thread::ThreadId>>,
    gate: Arc<(Mutex<GateState>, Condvar)>,
) {
    appender.arm_gate_acquired_hook({
        let appender = Arc::clone(&appender);
        let holder_thread = Arc::clone(&holder_thread);
        let gate = Arc::clone(&gate);
        Arc::new(move || {
            if !matches!(
                holder_thread.get(),
                Some(holder) if *holder == std::thread::current().id()
            ) {
                arm_checkpoint_holder_gate_hook(
                    Arc::clone(&appender),
                    Arc::clone(&holder_thread),
                    Arc::clone(&gate),
                );
                return;
            }
            {
                let (lock, condvar) = &*gate;
                let mut state = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.entered = true;
                condvar.notify_all();
            }
            wait_until(&gate, "gate release", |state| state.released);
        })
    });
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
    let mut state = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while !ready(*state) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (next, timed_out) = condvar
            .wait_timeout(state, remaining)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state = next;
        if timed_out.timed_out() && !ready(*state) {
            drop(state);
            panic!("{stage} did not happen within {CALL_TIMEOUT:?}");
        }
    }
}

/// Release the parked checkpoint. Called on every path that saw the gate
/// entered, so no failure can leave the checkpoint inside it.
fn release_gate(gate: &(Mutex<GateState>, Condvar)) {
    let (lock, condvar) = gate;
    let mut state = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.released = true;
    condvar.notify_all();
}

/// [`release_gate`] on `Drop`, so a panic between arming the gate hook and the
/// explicit release cannot wedge the parked checkpoint (and the suite) for the
/// rest of the process. Releasing is idempotent; the parked hook's own wait
/// stays bounded by [`CALL_TIMEOUT`] as a second line of defence.
struct GateRelease<'a>(&'a (Mutex<GateState>, Condvar));

impl Drop for GateRelease<'_> {
    fn drop(&mut self) {
        release_gate(self.0);
    }
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

/// Same as [`on_big_stack`], but with an explicitly BOUNDED blocking pool so a
/// test can occupy every blocking thread and observe what still runs.
fn on_big_stack_with_blocking_threads<F, Fut>(blocking_threads: usize, body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .max_blocking_threads(blocking_threads)
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
