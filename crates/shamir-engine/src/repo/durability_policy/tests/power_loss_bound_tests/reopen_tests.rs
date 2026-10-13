//! Real factory reopen regressions. Paused time, no detached spawns. The
//! one exception to "no sleeps" is the bounded reopen retry below, mirroring
//! `tx/tests/recovery_tests.rs::reopen_disk_repo` (lazy Windows file-lock
//! release after `drop`).

use std::path::Path;

use shamir_query_types::batch::VisibilityWait;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;
use shamir_wal::{SegmentSet, WalDurability, WalEntryV2};

use super::super::helpers::{self, DiskFixture};
use crate::repo::durability_policy::{
    DurabilityPolicy, ReplicationServe, RetentionOverflow, WalCommitTier,
    DEFAULT_WAL_RETENTION_MAX_BYTES,
};
use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;

async fn open(backing: &Path, name: &str) -> RepoInstance {
    RepoInstance::from_factory(
        name.into(),
        BoxRepoFactory::fjall_raw(backing.to_path_buf()),
        vec![TableConfig::new("items")],
    )
    .await
    .unwrap()
}

async fn next_commit_is_servable(repo: &RepoInstance, h: u64) {
    repo.set_durability(DurabilityPolicy::new(
        WalCommitTier::Synced,
        DEFAULT_WAL_RETENTION_MAX_BYTES,
        RetentionOverflow::RefuseWrites,
        ReplicationServe::Durable,
    ))
    .unwrap();
    let owned = repo.clone();
    let task = tokio::spawn(async move {
        let table = owned.get_table("items").await.unwrap();
        let (mut tx, pin) = owned.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        table
            .insert_tx(&InnerValue::Str("after-recovery".into()), Some(&mut tx))
            .await
            .unwrap();
        let info = owned
            .commit_tx_with_options(tx, VisibilityWait::Bounded { timeout_ms: 0 })
            .await
            .unwrap();
        drop(pin);
        info.commit_version
    });
    let v = task.await.unwrap();
    assert_eq!(v, h + 1);
    assert_eq!(repo.wal_group().await.unwrap().recoverable_through(), v);
    let (events, gap) = repo.read_changelog_for_replication(v, 100).await.unwrap();
    assert_eq!(gap, None);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.commit_version, v);
}

/// Bounded-retry reopen, mirroring `tx/tests/recovery_tests.rs::reopen_disk_repo`:
/// after an instance that performed real commits and truncation is dropped,
/// Windows releases the fjall file lock lazily, so the first reopen attempt
/// can fail with `FjallError: Locked` even though every background task has
/// exited (this same test's FIRST reopen, after three real commits, succeeds
/// with a plain `open` — no production task leaks the instance).
async fn open_retry(backing: &Path, name: &str) -> RepoInstance {
    let mut last_err = None;
    for _attempt in 0..10 {
        match RepoInstance::from_factory(
            name.into(),
            BoxRepoFactory::fjall_raw(backing.to_path_buf()),
            vec![TableConfig::new("items")],
        )
        .await
        {
            Ok(repo) => return repo,
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
    panic!("reopen of {name:?} failed after 10 retries: {:?}", last_err);
}

// Not `start_paused`: the reopen retry below needs REAL wall time for the
// OS to release the fjall file lock (see `open_retry`).
#[tokio::test]
async fn reopened_checkpointed_truncated_history_allows_next_commit() {
    let DiskFixture {
        repo,
        _dir,
        backing,
    } = helpers::disk_repo("truncated").await;
    for h in 0..3 {
        next_commit_is_servable(&repo, h).await;
    }
    repo.flush_buffers().await.unwrap();
    repo.changefeed()
        .await
        .unwrap()
        .feed
        .record_clean_horizon(3)
        .await
        .unwrap();
    repo.stop_journal_writer_for_test().await.unwrap();
    drop(repo);
    let segments = SegmentSet::open(backing.with_file_name("data.shamirwal"), 1)
        .await
        .unwrap();
    // Force a real rotation of the segment containing actual commits 1..3.
    // The extra eventless frame is recovered and certified conservatively.
    let entry = WalEntryV2::new(4, 0, Vec::new()).with_commit_version(4);
    segments
        .append_batch(vec![entry.encode().unwrap()], 4)
        .await
        .unwrap();
    segments.sync().await.unwrap();
    drop(segments);
    let repo = open(&backing, "truncated").await;
    assert_eq!(repo.tx_gate().await.unwrap().durable_watermark(), 4);
    assert_eq!(repo.changefeed().await.unwrap().feed.clean_horizon(), 4);
    // Same history-flush + checkpoint + real segment-unlink boundary; no
    // manual removal masquerading as history durability.
    repo.flush_all_history().await.unwrap();
    let wal = repo.repo_wal().await.unwrap();
    assert_eq!(wal.truncate_below(4).await.unwrap(), 1);
    assert!(wal.recover().await.unwrap().is_empty());
    drop(wal);
    repo.stop_journal_writer_for_test().await.unwrap();
    drop(repo);
    let reopened = open_retry(&backing, "truncated").await;
    assert_eq!(reopened.wal_group().await.unwrap().recoverable_through(), 4);
    next_commit_is_servable(&reopened, 4).await;
    reopened.stop_journal_writer_for_test().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn reopened_sparse_wal_settles_historical_abort_hole() {
    let DiskFixture {
        repo,
        _dir,
        backing,
    } = helpers::disk_repo("abort-hole").await;
    let gate = repo.tx_gate().await.unwrap();
    let guard = gate.assign_next_version_guarded();
    assert_eq!(guard.version(), 1);
    drop(guard); // actual aborted allocation: no WAL frame for version 1
    assert_eq!(gate.completion().watermark(), 1);
    helpers::append_wal(&repo, 2, WalDurability::Synced)
        .await
        .unwrap();
    assert_eq!(helpers::recovered_horizon(&repo).await, 2);
    drop(gate);
    repo.stop_journal_writer_for_test().await.unwrap();
    drop(repo);
    let reopened = open(&backing, "abort-hole").await;
    assert_eq!(reopened.wal_group().await.unwrap().recoverable_through(), 2);
    assert_eq!(
        reopened
            .changefeed()
            .await
            .unwrap()
            .feed
            .certified_gap_interval(),
        Some((1, 2)),
    );
    next_commit_is_servable(&reopened, 2).await;
    reopened.stop_journal_writer_for_test().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn reopened_all_aborted_wal_less_checkpoint_allows_next_commit() {
    let DiskFixture {
        repo,
        _dir,
        backing,
    } = helpers::disk_repo("all-aborted").await;
    let gate = repo.tx_gate().await.unwrap();
    for version in 1..=3 {
        let guard = gate.assign_next_version_guarded();
        assert_eq!(guard.version(), version);
        drop(guard);
    }
    assert_eq!(gate.completion().watermark(), 3);
    assert!(repo
        .repo_wal()
        .await
        .unwrap()
        .recover()
        .await
        .unwrap()
        .is_empty());
    repo.flush_all_history().await.unwrap();
    let feed = repo.changefeed().await.unwrap();
    feed.feed.record_recovery_gap(1, 3).await.unwrap();
    feed.feed.record_clean_horizon(3).await.unwrap();
    drop(feed);
    drop(gate);
    repo.stop_journal_writer_for_test().await.unwrap();
    drop(repo);
    let reopened = open(&backing, "all-aborted").await;
    assert!(reopened
        .repo_wal()
        .await
        .unwrap()
        .recover()
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        reopened
            .tx_gate()
            .await
            .unwrap()
            .version_allocation_high_water_mark(),
        3
    );
    assert_eq!(reopened.wal_group().await.unwrap().recoverable_through(), 3);
    next_commit_is_servable(&reopened, 3).await;
    reopened.stop_journal_writer_for_test().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn live_first_touch_does_not_certify_replayed_page_cache() {
    let DiskFixture {
        repo,
        _dir,
        backing,
    } = helpers::disk_repo("uncertified-touch").await;
    repo.stop_journal_writer_for_test().await.unwrap();
    drop(repo);
    let segments = SegmentSet::open(backing.with_file_name("data.shamirwal"), u64::MAX)
        .await
        .unwrap();
    let entry = WalEntryV2::new(1, 0, Vec::new()).with_commit_version(1);
    segments
        .append_batch(vec![entry.encode().unwrap()], 1)
        .await
        .unwrap();
    drop(segments); // bytes may exist only in page cache; no fsync
    let unrecovered = RepoInstance::from_factory_for_recovery_test(
        "uncertified-touch".into(),
        BoxRepoFactory::fjall_raw(backing),
        vec![TableConfig::new("items")],
    )
    .await
    .unwrap();
    assert_eq!(helpers::recovered_horizon(&unrecovered).await, 1);
    assert_eq!(
        unrecovered.wal_group().await.unwrap().recoverable_through(),
        0
    );
    unrecovered.recover_v2_inflight().await.unwrap();
    assert_eq!(
        unrecovered.wal_group().await.unwrap().recoverable_through(),
        1
    );
    unrecovered.stop_journal_writer_for_test().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn power_loss_real_reopen_bounds_survivors_and_next_synced_commit() {
    let fx = helpers::disk_repo("power-reopen").await;
    helpers::append_wal(&fx.repo, 1, WalDurability::Synced)
        .await
        .unwrap();
    let file = fx.wal_file();
    let safe_len = std::fs::metadata(&file).unwrap().len();
    fx.repo.stop_journal_writer_for_test().await.unwrap();
    let DiskFixture {
        repo,
        _dir,
        backing,
    } = fx;
    drop(repo);
    // Append directly through the real segment writer, without a timer which
    // could fsync the deliberately unsynced tail while paused time auto-runs.
    let segments = SegmentSet::open(backing.with_file_name("data.shamirwal"), u64::MAX)
        .await
        .unwrap();
    let lost = WalEntryV2::new(2, 0, Vec::new()).with_commit_version(2);
    segments
        .append_batch(vec![lost.encode().unwrap()], 2)
        .await
        .unwrap();
    assert!(std::fs::metadata(&file).unwrap().len() > safe_len);
    drop(segments);
    // Lose only the unsynced WAL bytes, BEFORE a fresh factory open. No live
    // page-cache replay is used as the recovered durability certificate.
    std::fs::OpenOptions::new()
        .write(true)
        .open(file)
        .unwrap()
        .set_len(safe_len)
        .unwrap();
    let reopened = open(&backing, "power-reopen").await;
    let h = helpers::recovered_horizon(&reopened).await;
    assert_eq!(h, 1);
    assert_eq!(reopened.wal_group().await.unwrap().recoverable_through(), h);
    let (events, gap) = reopened
        .read_changelog_for_replication(0, 100)
        .await
        .unwrap();
    assert!(events.iter().all(|event| event.event.commit_version <= h));
    assert_eq!(gap, Some(1)); // unknown old-format history stays conservative
    next_commit_is_servable(&reopened, h).await;
    reopened.stop_journal_writer_for_test().await.unwrap();
}
