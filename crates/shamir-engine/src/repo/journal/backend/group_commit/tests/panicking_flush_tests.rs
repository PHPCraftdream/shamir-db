use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::repo::group_commit::GroupCommit;

/// Regression for task group 10 (panic-safety audit): a panic inside
/// `flush()` must not strand `leader_busy = true` forever. Before the fix,
/// `leader_loop` only reset `leader_busy = false` on its normal exit path
/// (the bottom of the loop) — a panic inside `flush()` aborted the detached
/// leader task mid-flight and skipped that reset entirely, so EVERY
/// subsequent `run()` caller would push a oneshot and park with no leader
/// ever elected again (a durability-flush DoS, the same class the
/// cancellation fix in `leader_cancel_tests.rs` closed for caller-side
/// aborts, but here the flush itself is the failure).
///
/// After the fix, `leader_loop` wraps `flush()` in `catch_unwind`, so a
/// panic is converted into an `Err` delivered to every waiter in the
/// current batch (including the leader's own caller) and `leader_busy` is
/// released exactly like an ordinary flush error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn panicking_flush_does_not_strand_subsequent_calls() {
    let gc = Arc::new(GroupCommit::new());

    // The leader's own call panics inside `flush()`. It must still resolve
    // (not hang) — the leader loop's `catch_unwind` converts the panic into
    // an `Err` delivered to every waiter in the batch, including the leader
    // itself.
    let first = tokio::time::timeout(Duration::from_secs(5), {
        let gc = Arc::clone(&gc);
        async move {
            gc.run(|| async { panic!("boom: injected flush panic") })
                .await
        }
    })
    .await;

    assert!(
        first.is_ok(),
        "leader's own run() must resolve (not hang) even though its flush panicked"
    );
    let first_result = first.unwrap();
    assert!(
        first_result.is_err(),
        "a panicking flush must surface as an Err to the caller, got {:?}",
        first_result
    );

    // `first` resolving only means the leader SENT the reply — it does not
    // guarantee the leader has gone on to re-lock `state` and release
    // `leader_busy` (that happens strictly after the batch send). Without
    // waiting for it here, `second` below could race into the OUTGOING
    // leader's queue instead of electing a fresh one, and get served by a
    // follow-up round of the SAME panicking closure (an `Err`) — not a
    // hang, but a flaky assertion below unrelated to the bug under test.
    // Waiting for `is_leader_busy() == false` is itself a regression check:
    // pre-fix, `leader_busy` is stranded `true` forever and this times out.
    let settled = tokio::time::timeout(Duration::from_secs(5), {
        let gc = Arc::clone(&gc);
        async move {
            while gc.is_leader_busy().await {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    })
    .await;
    assert!(
        settled.is_ok(),
        "leader_busy did not settle to false within 5s after the panicking flush — \
         it is stranded (task group 10 regression)"
    );

    // A subsequent caller (simulating a new `synced_flush` arriving after the
    // panic) MUST complete within a bounded time and, since the leader has
    // now fully settled, MUST elect a fresh leader and actually succeed.
    let flush_count = Arc::new(AtomicUsize::new(0));
    let fc = Arc::clone(&flush_count);
    let second = tokio::time::timeout(Duration::from_secs(5), {
        let gc = Arc::clone(&gc);
        async move {
            gc.run(move || {
                let fc = Arc::clone(&fc);
                async move {
                    fc.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .await
        }
    })
    .await;

    assert!(
        second.is_ok(),
        "subsequent run() must complete within 5s after a panicking flush; \
         it hung — leader_busy was stranded (task group 10)"
    );
    let second_result = second.unwrap();
    assert!(
        second_result.is_ok(),
        "subsequent run() should succeed: {:?}",
        second_result
    );
    assert_eq!(
        flush_count.load(Ordering::SeqCst),
        1,
        "the subsequent flush must have run exactly once"
    );
}
