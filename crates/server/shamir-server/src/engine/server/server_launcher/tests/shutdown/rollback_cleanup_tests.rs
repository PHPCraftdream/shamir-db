//! A failed launch's cleanup must be REPORTED, never silently dropped: every
//! audit/repository failure the shared teardown recorded is logged at error
//! level by [`log_rollback_cleanup`], and the boot error still wins.
//!
//! Mutant killed: the function's body removed, or its `tracing::error!` level
//! lowered to debug — the ring captures debug too, so the level assertion is
//! what fails, not only the "no record at all" case.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use shamir_connect::server::lockout::InMemoryLockoutStore;
use shamir_connect::server::rate_limit::InMemoryRateLimiter;
use shamir_db::ShutdownReport;
use shamir_test_diag::{ring::shared_ring, try_install_global_tracing_ring};
use tokio_util::sync::CancellationToken;

use super::rollback_cleanup::{join_meta_snapshot, log_rollback_cleanup};
use crate::server::meta_sinks::spawn_meta_snapshot_task;

#[test]
fn launch_rollback_cleanup_logs_errors_and_repository_failures() {
    let _ = try_install_global_tracing_ring();

    // Both failures the report can carry, with inputs unique to this test so
    // no other process-wide log line can satisfy the assertions.
    let mut cleanup = ShutdownReport::default();
    cleanup.add_error("test rollback error alpha");
    cleanup.record(
        "test-repo-beta",
        Err("test repository failure gamma".to_string()),
    );
    log_rollback_cleanup("test-rollback-stage", &cleanup);

    let records = shared_ring().snapshot();
    let stage_field = "stage=\"test-rollback-stage\"";
    let events: Vec<_> = records
        .iter()
        .filter(|record| {
            record.message.contains("launch rollback cleanup error")
                && record.message.contains(stage_field)
        })
        .collect();
    assert!(
        events.iter().any(|record| record.level == "ERROR"
            && record
                .message
                .contains("error=\"test rollback error alpha\"")),
        "every rollback cleanup error must be logged at ERROR for the exact input: {records:?}"
    );
    assert!(
        events.iter().any(|record| record.level == "ERROR"
            && record.message.contains("repository=\"test-repo-beta\"")
            && record
                .message
                .contains("error=\"test repository failure gamma\"")),
        "every rollback repository failure must be logged at ERROR for the exact input: \
         {records:?}"
    );
}

/// Polls a future exactly once without waiting for it.
async fn poll_once<F: Future<Output = ()> + Unpin>(future: &mut F) -> Poll<()> {
    std::future::poll_fn(|cx| Poll::Ready(Pin::new(&mut *future).poll(cx))).await
}

/// The meta-snapshot join of a failed launch is bounded: the task's final
/// snapshot is a blocking-pool job, so with the pool saturated an unbounded
/// `handle.await` never returned the boot error. The pool's only blocking thread
/// is parked before the task is told to stop; the join must stay pending just
/// before its 5 s bound, return once the bound passed, log the expiry for its
/// stage, and return while the pool is still occupied.
///
/// The clock is PAUSED and advanced explicitly (no real-time wait, no race): the
/// snapshot task is the only runnable task, and it is given several scheduler
/// turns to run up to its queued blocking job before the join is first polled.
///
/// Mutants: the bound removed (`task.handle.await`) leaves the join pending
/// after the bound and fails the last poll; a shorter bound completes it before
/// the bound and fails the middle poll; a text that does not name the stage or
/// the cause fails the log assertion.
#[test]
fn rollback_meta_snapshot_join_is_bounded_with_a_saturated_blocking_pool() {
    let _ = try_install_global_tracing_ring();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .max_blocking_threads(1)
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let occupant_done = Arc::new(AtomicBool::new(false));
        let job_done = Arc::clone(&occupant_done);
        // Detached on purpose: the sender's drop releases the job.
        drop(tokio::task::spawn_blocking(move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
            job_done.store(true, Ordering::SeqCst);
        }));
        started_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("the occupying job must start on the only blocking thread");

        let shutdown = CancellationToken::new();
        let task = spawn_meta_snapshot_task(
            Arc::new(InMemoryLockoutStore::new()),
            Arc::new(InMemoryRateLimiter::new(0)),
            shutdown.clone(),
        );
        shutdown.cancel();
        // The task's first tick is immediate and the cancel wins its select: a few
        // turns take it up to the final snapshot job queued behind the occupant.
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }

        let mut join = Box::pin(join_meta_snapshot("test-rollback-meta-join", Some(task)));
        assert!(
            poll_once(&mut join).await.is_pending(),
            "the join must wait for the stuck task"
        );
        tokio::time::advance(Duration::from_millis(4_990)).await;
        assert!(
            poll_once(&mut join).await.is_pending(),
            "the join must still be waiting just before its 5 s bound"
        );
        tokio::time::advance(Duration::from_millis(20)).await;
        let returned = poll_once(&mut join).await;
        let still_occupied = !occupant_done.load(Ordering::SeqCst);
        drop(release_tx);
        assert!(
            returned.is_ready(),
            "the join must return once its bound passed, with the pool saturated"
        );
        assert!(
            still_occupied,
            "the join must return while the pool is still occupied"
        );

        let records = shared_ring().snapshot();
        assert!(
            records.iter().any(|record| record.level == "ERROR"
                && record.message.contains("launch rollback cleanup error")
                && record.message.contains("stage=\"test-rollback-meta-join\"")
                && record
                    .message
                    .contains("error=\"meta snapshot task did not stop within 5s")),
            "the expired join must be logged at ERROR: {records:?}"
        );
    });
}
