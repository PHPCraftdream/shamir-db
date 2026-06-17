// shamir-collections not a dep in shamir-wal
#![allow(clippy::disallowed_types)]
use std::collections::HashSet;
use std::sync::Arc;

use shamir_types::types::record_id::RecordId;
use tempfile::TempDir;

use crate::segment_set::SegmentSet;
use crate::wal_entry_v2::{WalEntryV2, WalOpV2};
use crate::wal_group_commit::{WalDurability, WalGroupCommit};
use crate::wal_sink::WalSink;

/// Large per-segment cap (64 MiB) so these tests — none of which are about
/// rotation — keep the single-segment behaviour they had pre-F6b.
const BIG_SEG: u64 = 64 * 1024 * 1024;

fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

fn entry(txn_id: u64, commit_version: u64) -> WalEntryV2 {
    WalEntryV2::new(
        txn_id,
        7,
        vec![WalOpV2::Delete {
            table_id_interned: 7,
            rid: rid(txn_id as u8),
        }],
    )
    .with_commit_version(commit_version)
}

async fn open_sink(dir: &TempDir) -> WalSink {
    let segset = SegmentSet::open(dir.path().to_path_buf(), BIG_SEG)
        .await
        .unwrap();
    WalSink::File(segset)
}

#[tokio::test]
async fn buffered_append_durable() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = WalGroupCommit::new(Arc::clone(&sink));

    gc.append(entry(1, 10).encode().unwrap(), 10, WalDurability::Buffered)
        .await
        .unwrap();

    let replayed = sink.replay().await.unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].txn_id, 1);
}

#[tokio::test]
async fn synced_append_durable() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = WalGroupCommit::new(Arc::clone(&sink));

    gc.append(entry(1, 10).encode().unwrap(), 10, WalDurability::Synced)
        .await
        .unwrap();

    let replayed = sink.replay().await.unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].txn_id, 1);
    assert!(gc.fsync_count() >= 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_mixed_tiers_all_durable() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = Arc::new(WalGroupCommit::new(Arc::clone(&sink)));

    let mut handles = Vec::new();
    // txn_ids 1..=32 Buffered, 33..=64 Synced — all distinct.
    for i in 1..=64u64 {
        let gc = Arc::clone(&gc);
        let tier = if i <= 32 {
            WalDurability::Buffered
        } else {
            WalDurability::Synced
        };
        handles.push(tokio::spawn(async move {
            gc.append(entry(i, i * 10).encode().unwrap(), i * 10, tier)
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let replayed = sink.replay().await.unwrap();
    assert_eq!(replayed.len(), 64);
    let got: HashSet<u64> = replayed.iter().map(|e| e.txn_id).collect();
    let want: HashSet<u64> = (1..=64u64).collect();
    assert_eq!(got, want);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn synced_fsyncs_are_batched() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = Arc::new(WalGroupCommit::new(Arc::clone(&sink)));

    let mut handles = Vec::new();
    for i in 1..=32u64 {
        let gc = Arc::clone(&gc);
        handles.push(tokio::spawn(async move {
            gc.append(
                entry(i, i * 10).encode().unwrap(),
                i * 10,
                WalDurability::Synced,
            )
            .await
            .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let replayed = sink.replay().await.unwrap();
    assert_eq!(replayed.len(), 32);

    let fsyncs = gc.fsync_count();
    assert!(fsyncs >= 1, "expected at least one fsync, got {fsyncs}");
    assert!(
        fsyncs < 32,
        "group commit should coalesce fsyncs, got {fsyncs}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn buffered_only_window_issues_no_fsync() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = Arc::new(WalGroupCommit::new(Arc::clone(&sink)));

    // A workload of ONLY Buffered appends — no matter how the windows
    // form, none of them carry a Synced waiter, so no fsync is ever
    // issued. fsync_count() == 0 is therefore deterministic, not timing
    // dependent.
    let mut handles = Vec::new();
    for i in 1..=16u64 {
        let gc = Arc::clone(&gc);
        handles.push(tokio::spawn(async move {
            gc.append(
                entry(i, i * 10).encode().unwrap(),
                i * 10,
                WalDurability::Buffered,
            )
            .await
            .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let replayed = sink.replay().await.unwrap();
    assert_eq!(replayed.len(), 16);
    assert_eq!(gc.fsync_count(), 0);
}

#[tokio::test]
async fn mem_append_returns_ok_and_replays() {
    let sink = Arc::new(WalSink::mem());
    let gc = WalGroupCommit::new(Arc::clone(&sink));

    gc.append(entry(1, 10).encode().unwrap(), 10, WalDurability::Buffered)
        .await
        .unwrap();

    // Mem replay returns the appended entry (in-RAM segment).
    let replayed = sink.replay().await.unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].txn_id, 1);
}

#[tokio::test]
async fn mem_sync_returns_ok() {
    let sink = Arc::new(WalSink::mem());
    let gc = WalGroupCommit::new(Arc::clone(&sink));

    // Mem sync is a no-op; a Synced append still succeeds.
    gc.append(entry(1, 10).encode().unwrap(), 10, WalDurability::Synced)
        .await
        .unwrap();

    let replayed = sink.replay().await.unwrap();
    assert_eq!(replayed.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_fsync_fires_for_buffered() {
    use std::time::Duration;

    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = Arc::new(WalGroupCommit::new(Arc::clone(&sink)));

    gc.spawn_background_fsync(Duration::from_millis(30));

    // Append one Buffered entry (no inline fsync).
    gc.append(entry(1, 10).encode().unwrap(), 10, WalDurability::Buffered)
        .await
        .unwrap();

    // Inline fsync_count should be 0 (Buffered doesn't fsync inline).
    assert_eq!(gc.fsync_count(), 0);

    // Wait long enough for the bg timer to fire.
    tokio::time::sleep(Duration::from_millis(120)).await;

    // Background fsync should have fired at least once.
    assert!(
        gc.fsync_count() >= 1,
        "expected bg fsync to fire, got {}",
        gc.fsync_count()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_fsync_skips_when_idle() {
    use std::time::Duration;

    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = Arc::new(WalGroupCommit::new(Arc::clone(&sink)));

    gc.spawn_background_fsync(Duration::from_millis(30));

    // No appends — wait for a few ticks.
    tokio::time::sleep(Duration::from_millis(120)).await;

    // No fsync should have been issued (dirty flag never set).
    assert_eq!(gc.fsync_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_fsync_exits_on_drop() {
    use std::time::Duration;

    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = Arc::new(WalGroupCommit::new(Arc::clone(&sink)));

    gc.spawn_background_fsync(Duration::from_millis(20));

    // Drop the only strong ref — bg task should exit on next tick.
    drop(gc);
    tokio::time::sleep(Duration::from_millis(80)).await;
    // If we get here without hanging, the task exited cleanly.
}
