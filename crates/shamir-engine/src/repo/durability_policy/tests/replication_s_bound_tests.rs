//! S bound in the replication read path (PR1 I3, live-path behaviour).
//!
//! With a File-sink WAL carrying an unsynced tail, a pull returns only
//! versions at or below `S`; once the versions are fsynced (Synced appends
//! fsync their window, raising `S`), the same pull includes them. The only
//! wait is the pull path's own bounded sync wait — no sleeps, and every
//! post-condition is asserted on the condition itself.

use shamir_wal::{WalDurability, WalGroupCommit, WalSink};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::repo::durability_policy::replication_bound::BOUND_WAIT_MS_FOR_TEST;

use super::super::replication_bound;
use super::super::RetentionOverflow;
use super::super::DEFAULT_WAL_RETENTION_MAX_BYTES;
use super::super::{DurabilityPolicy, RecoverabilityCoverage, ReplicationServe};
use shamir_wal::wal_group_commit::WalCoverage;

use super::helpers;

#[tokio::test]
async fn pull_holds_back_versions_above_s_until_they_are_synced() {
    let fx = helpers::disk_repo("s_bound_replication").await;
    helpers::seed_journal(&fx.repo, &[1, 2, 3]).await;
    // v1 fsynced (its Synced window raised S to 1); v2/v3 not yet written
    // to the WAL at all, so they must NOT be served.
    helpers::append_wal(&fx.repo, 1, WalDurability::Synced)
        .await
        .unwrap();

    let (events, gap_at) = fx
        .repo
        .read_changelog_for_replication(0, 100)
        .await
        .unwrap();
    let served = events
        .iter()
        .map(|e| e.event.commit_version)
        .collect::<Vec<_>>();
    assert_eq!(served, vec![1], "versions above S must be held back");
    assert_eq!(gap_at, None);

    // Synced appends fsync their own group-commit window: S advances to 3
    // deterministically (wait on the condition, not on time).
    helpers::append_wal(&fx.repo, 2, WalDurability::Synced)
        .await
        .unwrap();
    helpers::append_wal(&fx.repo, 3, WalDurability::Synced)
        .await
        .unwrap();
    assert_eq!(fx.repo.wal_group().await.unwrap().recoverable_through(), 3);

    let (events, _) = fx
        .repo
        .read_changelog_for_replication(0, 100)
        .await
        .unwrap();
    let served = events
        .iter()
        .map(|e| e.event.commit_version)
        .collect::<Vec<_>>();
    assert_eq!(served, vec![1, 2, 3], "synced versions are now servable");
}

/// B3 point 5 (unified coordinator): many concurrent held-back pulls cause
/// exactly ONE `sync_now` leader round — coalescing ACROSS pulls, not just
/// within one `sync_now` call. Clear only timer admission, then park the
/// shared fsync until all pull demands register. Assert the repository-local
/// round counter delta; spawned tasks are joined.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_held_back_pulls_coalesce_into_one_sync() {
    let fx = helpers::disk_repo("pull_coalesce").await;
    helpers::seed_journal(&fx.repo, &[1, 2, 3]).await;
    // v1 fsynced (R = 1); v2/v3 have only buffered WAL bytes — every
    // pull's bound (3) exceeds R, so each pull demands a sync.
    helpers::append_wal(&fx.repo, 1, WalDurability::Synced)
        .await
        .unwrap();
    helpers::append_wal(&fx.repo, 2, WalDurability::Buffered)
        .await
        .unwrap();
    helpers::append_wal(&fx.repo, 3, WalDurability::Buffered)
        .await
        .unwrap();

    // Clear only timer admission; do NOT sync before the pulls. A pre-sync
    // would make the held-back observation vacuous.
    let group = fx.repo.wal_group().await.unwrap();
    let _ = group.take_dirty();
    let rounds0 = group.instance_sync_rounds();
    let demands0 = group.instance_sync_calls();

    // Park the (single) coalesced round mid-fsync so all 8 pulls are
    // guaranteed to register inside the SAME window (no wall clock). The
    // 200 ms pull budget would otherwise expire early waiters before the
    // last one registers on a slow runner, so `wait_sync_waiters(8)` could
    // never observe 8 at once; the budget is lifted for this test only.
    BOUND_WAIT_MS_FOR_TEST.store(3_600_000, Ordering::Release);
    shamir_wal::WalSink::arm_pause_next_file_sync();
    let mut pulls = Vec::new();
    for _ in 0..8 {
        let repo = fx.repo.clone();
        pulls.push(tokio::spawn(async move {
            repo.read_changelog_for_replication(0, 100).await.unwrap()
        }));
    }
    shamir_wal::WalSink::wait_file_sync_paused().await;
    group.wait_sync_waiters(8).await;
    assert_eq!(group.instance_sync_calls() - demands0, 8);
    shamir_wal::WalSink::resume_file_sync();

    let mut served_max = 0;
    for handle in pulls {
        let (events, _) = handle.await.unwrap();
        for e in &events {
            served_max = served_max.max(e.event.commit_version);
        }
    }
    // Every pull was held back to R, then the single coalesced round raised
    // R to cover the demanded bound.
    let rounds1 = group.instance_sync_rounds();
    assert_eq!(rounds1 - rounds0, 1, "same captured versions need one sync");
    assert_eq!(group.recoverable_through(), 3);
    assert_eq!(served_max, 3);
}

/// A stalled disk is bounded by the pull budget, not an unbounded sync await.
/// Paused Tokio time advances to the deadline while the File sync remains
/// parked; finishing the demander cannot cancel the worker's fsync.
#[tokio::test(start_paused = true)]
async fn pull_deadline_does_not_cancel_shared_sync() {
    let dir = tempfile::TempDir::new().unwrap();
    let segments = shamir_wal::SegmentSet::open(dir.path().to_path_buf(), 64 * 1024 * 1024)
        .await
        .unwrap();
    let group = Arc::new(WalGroupCommit::new(Arc::new(WalSink::File(segments))));
    WalSink::arm_pause_next_file_sync();
    let pull = {
        let group = Arc::clone(&group);
        tokio::spawn(async move { replication_bound(&group, 1, Duration::from_millis(200)).await })
    };
    WalSink::wait_file_sync_paused().await;
    tokio::time::advance(Duration::from_millis(200)).await;
    assert_eq!(pull.await.unwrap(), 0);
    assert_eq!(group.sync_waiters(), 0);
    let remaining = {
        let group = Arc::clone(&group);
        tokio::spawn(async move { group.sync_now().await })
    };
    group.wait_sync_waiters(1).await;
    WalSink::resume_file_sync();
    remaining.await.unwrap().unwrap();
}

#[test]
fn exact_coverage_preserves_holes_and_capture_boundary() {
    let coverage = RecoverabilityCoverage::new(3);
    coverage.written(&[5], false);
    let first = coverage.capture();
    coverage.written(&[6], false); // arrived after first capture
    coverage.synced(first);
    assert_eq!(coverage.recoverable_through(), 3);
    coverage.settle(9); // high settlement cannot invent predecessors
    assert_eq!(coverage.recoverable_through(), 3);
    coverage.settle(4);
    assert_eq!(coverage.recoverable_through(), 5);
    coverage.synced(coverage.capture());
    assert_eq!(coverage.recoverable_through(), 6);
    coverage.settle(8);
    assert_eq!(coverage.recoverable_through(), 6);
    coverage.settle(7);
    assert_eq!(coverage.recoverable_through(), 9);
}

#[tokio::test]
async fn appended_five_before_four_stays_below_hole_after_real_sync() {
    let dir = tempfile::TempDir::new().unwrap();
    let segments = shamir_wal::SegmentSet::open(dir.path().to_path_buf(), 64 * 1024 * 1024)
        .await
        .unwrap();
    let group = WalGroupCommit::new(Arc::new(WalSink::File(segments)));
    group
        .install_coverage(Arc::new(RecoverabilityCoverage::new(3)))
        .unwrap();
    let entry = shamir_wal::WalEntryV2::new(5, 0, Vec::new()).with_commit_version(5);
    group
        .append(entry.encode().unwrap(), 5, WalDurability::Buffered)
        .await
        .unwrap();
    group.sync_now().await.unwrap();
    assert_eq!(group.recoverable_through(), 3);
    group.settle(4);
    assert_eq!(group.recoverable_through(), 5);
}

/// A limited durable page never waits for an unrelated unsynced tail.
#[tokio::test]
async fn durable_limited_page_issues_no_tail_sync() {
    let fx = helpers::disk_repo("durable_page_no_tail_sync").await;
    helpers::seed_journal(&fx.repo, &[1, 2]).await;
    helpers::append_wal(&fx.repo, 1, WalDurability::Synced)
        .await
        .unwrap();
    helpers::append_wal(&fx.repo, 2, WalDurability::Buffered)
        .await
        .unwrap();
    let group = fx.repo.wal_group().await.unwrap();
    let _ = group.take_dirty();
    let calls = group.instance_sync_calls();
    let rounds = group.instance_sync_rounds();
    let (events, gap) = fx.repo.read_changelog_for_replication(0, 1).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.commit_version, 1);
    assert_eq!(gap, None);
    assert_eq!(group.instance_sync_calls(), calls);
    assert_eq!(group.instance_sync_rounds(), rounds);
    assert_eq!(group.recoverable_through(), 1);
}

/// Completion abort callbacks must wake a registered recoverability waiter.
#[tokio::test]
async fn abort_hole_wakes_waiter_without_another_sync() {
    use std::future::Future;
    use std::task::Poll;
    let fx = helpers::disk_repo("abort_wake").await;
    let gate = fx.repo.tx_gate().await.unwrap();
    let guard = gate.assign_next_version_guarded();
    assert_eq!(guard.version(), 1);
    helpers::append_wal(&fx.repo, 2, WalDurability::Synced)
        .await
        .unwrap();
    let group = fx.repo.wal_group().await.unwrap();
    assert_eq!(group.recoverable_through(), 0);
    let wait = group.wait_recoverable(2, Duration::from_secs(10));
    tokio::pin!(wait);
    std::future::poll_fn(|cx| {
        assert!(wait.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    let rounds = group.instance_sync_rounds();
    drop(guard);
    assert!(wait.await);
    assert_eq!(group.recoverable_through(), 2);
    assert_eq!(group.instance_sync_rounds(), rounds);
    fx.repo.stop_journal_writer_for_test().await.unwrap();
}

/// PR8 chunk 1: a `completed_reseed` policy changes ONLY the serve mode —
/// the other accessors (`wal_commit`, `effective_wal_commit`, retention
/// budget, overflow action) report exactly what was constructed.
#[test]
fn completed_reseed_policy_keeps_other_accessors_unchanged() {
    let policy = DurabilityPolicy::new(
        shamir_query_types::WalCommitTier::Synced,
        DEFAULT_WAL_RETENTION_MAX_BYTES,
        RetentionOverflow::DeclareGap,
        ReplicationServe::CompletedReseed,
    );
    assert_eq!(
        policy.replication_serve(),
        ReplicationServe::CompletedReseed
    );
    assert_eq!(
        policy.wal_commit(),
        shamir_query_types::WalCommitTier::Synced
    );
    assert_eq!(
        policy.effective_wal_commit(),
        shamir_query_types::WalCommitTier::Synced
    );
    assert_eq!(
        policy.wal_retention_max_bytes(),
        DEFAULT_WAL_RETENTION_MAX_BYTES
    );
    assert_eq!(
        policy.on_wal_retention_exceeded(),
        RetentionOverflow::DeclareGap
    );
}

/// PR8 chunk 3 fixture: journal with completion watermark W = 3, WAL
/// containing ONLY v1 (fsynced) — so the recoverability prefix R = 1 is
/// strictly below W = 3. The policy mode is set through `set_durability`
/// with `wal_commit = buffered` (buffered is valid on a File backend and
/// nothing in the read path depends on the commit tier).
async fn serve_fixture(name: &str, mode: ReplicationServe) -> helpers::DiskFixture {
    let fx = helpers::disk_repo(name).await;
    helpers::seed_journal(&fx.repo, &[1, 2, 3]).await;
    helpers::append_wal(&fx.repo, 1, WalDurability::Synced)
        .await
        .unwrap();
    fx.repo
        .set_durability(DurabilityPolicy::new(
            shamir_query_types::WalCommitTier::Buffered,
            DEFAULT_WAL_RETENTION_MAX_BYTES,
            RetentionOverflow::DeclareGap,
            mode,
        ))
        .unwrap();
    fx
}

/// PR8 chunk 3 (`replication_serve = completed_reseed`): a pull is bounded
/// by `min(confirmed, W)` — NO recoverability clamp — so it serves the
/// journal completion prefix `1..=3` even though only v1 survives a power
/// loss (R = 1). This test is the NEGATIVE CONTROL of
/// [`durable_mode_still_clamps_to_recoverable_prefix`] (same fixture state,
/// opposite mode): on the pre-chunk-3 code the mode is ignored and the
/// durable R clamp holds this pull back to `[1]`, so the final assertion
/// fails.
#[tokio::test]
async fn completed_reseed_serves_past_recoverable_prefix() {
    let fx = serve_fixture("reseed_serves_past_r", ReplicationServe::CompletedReseed).await;
    assert_eq!(fx.repo.wal_group().await.unwrap().recoverable_through(), 1);
    let (events, gap) = fx
        .repo
        .read_changelog_for_replication(0, 100)
        .await
        .unwrap();
    assert_eq!(gap, None);
    let served = events
        .iter()
        .map(|e| e.event.commit_version)
        .collect::<Vec<_>>();
    assert_eq!(
        served,
        vec![1, 2, 3],
        "completed_reseed serves up to the confirm/W bound, past R"
    );
}

/// PR8 chunk 3: `durable` (the default) serving is UNCHANGED — the same
/// fixture state as [`completed_reseed_serves_past_recoverable_prefix`]
/// (which is this test's negative control) under mode `Durable` stops
/// exactly at the recoverability prefix R = 1. Pins today's behaviour.
#[tokio::test]
async fn durable_mode_still_clamps_to_recoverable_prefix() {
    let fx = serve_fixture("durable_clamps_to_r", ReplicationServe::Durable).await;
    assert_eq!(fx.repo.wal_group().await.unwrap().recoverable_through(), 1);
    let (events, gap) = fx
        .repo
        .read_changelog_for_replication(0, 100)
        .await
        .unwrap();
    assert_eq!(gap, None);
    let served = events
        .iter()
        .map(|e| e.event.commit_version)
        .collect::<Vec<_>>();
    assert_eq!(
        served,
        vec![1],
        "durable serving still clamps to the recoverability prefix"
    );
}
