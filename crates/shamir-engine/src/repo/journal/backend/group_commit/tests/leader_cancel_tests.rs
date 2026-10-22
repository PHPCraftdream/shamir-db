use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;

use crate::repo::group_commit::GroupCommit;

/// Regression for audit §2.1 (top-5 #5): if the leader's caller future is
/// cancelled (the caller's task is dropped — client disconnect, `select!`
/// race, graceful shutdown) while a flush is in-flight, `leader_busy` MUST
/// NOT stay `true` forever, because that would park every subsequent
/// `synced_flush` on this repo indefinitely (a durability-flush DoS
/// triggered by one dropped caller).
///
/// Before the fix, `flush().await` ran INLINE on the caller's task, so
/// cancelling the caller cancelled the flush mid-flight and stranded
/// `leader_busy = true` forever. After the fix, the leader loop runs in a
/// DETACHED `tokio::task`, so cancelling the caller only abandons the
/// caller's `oneshot` wait — the spawned leader completes the flush
/// normally and releases `leader_busy`.
///
/// This test cancels a leader caller mid-flush (while the flush body is
/// still running), then asserts a subsequent `run()` completes within a
/// bounded time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leader_cancellation_does_not_strand_subsequent_calls() {
    let gc = Arc::new(GroupCommit::new());
    let flush_started = Arc::new(tokio::sync::Notify::new());
    let flush_count = Arc::new(AtomicUsize::new(0));

    // Spawn the leader call in its own task so we can abort it (cancel the
    // caller) while the flush is in-flight. The flush body sleeps long enough
    // to observe the cancellation window, but COMPLETES (it is not pending
    // forever) — mirroring a real flush+fsync that the detached leader must
    // be allowed to finish even after the caller is gone.
    let mut js = JoinSet::new();
    let gc1 = Arc::clone(&gc);
    let fs1 = Arc::clone(&flush_started);
    let fc1 = Arc::clone(&flush_count);
    let leader_handle = js.spawn(async move {
        gc1.run(move || {
            let fs = Arc::clone(&fs1);
            let fc = Arc::clone(&fc1);
            async move {
                fc.fetch_add(1, Ordering::SeqCst);
                // Signal that the flush body has begun.
                fs.notify_one();
                // Hold the flush open long enough to cancel the caller
                // mid-flight. The detached leader (post-fix) completes this
                // sleep and releases leadership; the pre-fix inline leader
                // is dropped here, stranding leader_busy.
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok(())
            }
        })
        .await
    });

    // Wait until the leader is actually inside the flush body.
    flush_started.notified().await;
    // Give the runtime a beat to settle.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Cancel the leader CALLER mid-flush. Before the fix, this drops the
    // inline `flush().await` future and strands `leader_busy = true` forever.
    leader_handle.abort();
    // Reap the aborted task.
    let _ = js.join_next().await;

    // A subsequent caller (simulating a new synced_flush arriving after the
    // cancellation) MUST complete within a bounded time. Before the fix this
    // hangs forever — `leader_busy` is stuck and no leader ever runs again.
    // After the fix, the detached leader finishes the in-flight flush, then
    // either serves this caller in a follow-up round or a fresh leader is
    // elected.
    let gc2 = Arc::clone(&gc);
    let fc2 = Arc::clone(&flush_count);
    let second = tokio::time::timeout(Duration::from_secs(5), async move {
        gc2.run(move || {
            let fc = Arc::clone(&fc2);
            async move {
                fc.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .await
    })
    .await;

    assert!(
        second.is_ok(),
        "subsequent run() must complete within 5s after leader caller cancellation; \
         it hung — leader_busy was stranded (audit §2.1)"
    );
    let result = second.unwrap();
    assert!(
        result.is_ok(),
        "subsequent run() should succeed: {:?}",
        result
    );
    // At least one flush ran for the second caller (the first caller's flush
    // may or may not have completed depending on timing, but the second
    // caller's flush definitely ran).
    assert!(
        flush_count.load(Ordering::SeqCst) >= 2,
        "a flush must have run for the second caller; got {}",
        flush_count.load(Ordering::SeqCst)
    );
}
