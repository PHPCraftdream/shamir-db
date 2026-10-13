//! Replication bound helper — the PR1 I3 invariant, one small function.
//!
//! A replication pull's bound (today `min(confirmed_through, W)`, computed
//! in `RepoInstance::read_changelog_bounded_impl`) must never serve a
//! version outside the WAL's contiguous recoverability prefix `R`: the
//! follower persists a bookmark for what it pulls, and a bookmark above `R`
//! would point past what a power loss can take away on the leader. This
//! helper clamps the bound to `R` ([`WalGroupCommit::recoverable_through`]),
//! and when the bound exceeds `R` triggers the COALESCED `sync_now` (the
//! ONE unified coordinator — concurrent pullers, the 250 ms timer and
//! `synced` commits all share its rounds) and does a bounded
//! `wait_recoverable` (usually well below the 250 ms background fsync
//! interval) before re-sampling `R`. Serve bound = `min(confirmed, W, R)`.

use std::time::Duration;

use shamir_wal::WalGroupCommit;

use super::DurabilityMetrics;

/// Default bounded wait for the pull-path sync: short by design — a quiet
/// sink completes its fsync in microseconds; a busy one is bounded so a
/// stalled disk degrades to an older (still safe) bound instead of hanging
/// the pull.
pub const REPLICATION_BOUND_WAIT: Duration = Duration::from_millis(200);

/// Test-only seam for the NEGATIVE CONTROL test: when set, the helper skips
/// the `R` clamp entirely and returns the unclamped bound, exercising the
/// UNPROTECTED pre-PR1 path so the test can prove the bound is load-bearing.
/// Nextest isolates each test in its own process, so this never leaks
/// between tests.
#[cfg(test)]
pub(crate) static SKIP_S_CLAMP_FOR_TEST: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Final replication bound: `min(bound, R)`.
///
/// When `bound > R`, first trigger the coalesced `sync_now` and wait
/// (bounded by `max_wait`) for `R` to reach `bound`; then re-sample `R` and
/// take the min. A failed fsync leaves `R` where it was — the returned
/// bound stays safe. On a `Mem` sink `R == W` after completion, so this returns
/// `bound` unchanged and never syncs (the invariant is free there).
/// cancel-safe: yes — deadline drops only a waiter, never shared sync work.
pub async fn replication_bound(group: &WalGroupCommit, bound: u64, max_wait: Duration) -> u64 {
    replication_bound_impl(group, bound, max_wait, None).await
}

/// Repository-labelled observation; same bound logic as the standalone helper.
/// cancel-safe: yes — dropping a waiter records duration, not shared I/O fate.
pub(crate) async fn replication_bound_observed(
    group: &WalGroupCommit,
    bound: u64,
    max_wait: Duration,
    metrics: &DurabilityMetrics,
) -> u64 {
    replication_bound_impl(group, bound, max_wait, Some(metrics)).await
}

async fn replication_bound_impl(
    group: &WalGroupCommit,
    bound: u64,
    max_wait: Duration,
    metrics: Option<&DurabilityMetrics>,
) -> u64 {
    #[cfg(test)]
    if SKIP_S_CLAMP_FOR_TEST.load(std::sync::atomic::Ordering::Acquire) {
        return bound;
    }
    if bound == 0 {
        if let Some(metrics) = metrics {
            metrics.observe_bound(0, 0);
        }
        return 0;
    }
    let r0 = group.recoverable_through();
    if let Some(metrics) = metrics {
        metrics.observe_bound(bound, bound.min(r0));
    }
    if bound <= r0 {
        return bound;
    }
    let _held = metrics.map(DurabilityMetrics::hold_pull);
    // One budget includes demand registration, fsync wait AND prefix wait.
    // The worker outlives timeout/cancellation; never await fsync unbounded.
    let _ = tokio::time::timeout(max_wait, async {
        if group.sync_now().await.is_ok() {
            let _ = group.wait_recoverable(bound, max_wait).await;
        }
    })
    .await;
    let r = group.recoverable_through();
    let served = r.min(bound);
    if let Some(metrics) = metrics {
        metrics.observe_bound(bound, served);
    }
    served
}
