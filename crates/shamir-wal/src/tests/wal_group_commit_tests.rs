use std::collections::HashSet;
use std::sync::Arc;

use shamir_types::types::record_id::RecordId;
use tempfile::TempDir;

use crate::wal_entry_v2::{WalEntryV2, WalOpV2};
use crate::wal_group_commit::{WalDurability, WalGroupCommit};
use crate::wal_segment::WalSegment;
use crate::wal_sink::WalSink;

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

fn seg_path(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("segment.wal")
}

#[tokio::test]
async fn buffered_append_durable() {
    let dir = TempDir::new().unwrap();
    let seg = WalSegment::open(seg_path(&dir)).await.unwrap();
    let sink = Arc::new(WalSink::File(seg));
    let gc = WalGroupCommit::new(Arc::clone(&sink));

    gc.append(entry(1, 10).encode().unwrap(), WalDurability::Buffered)
        .await
        .unwrap();

    let replayed = sink.replay().await.unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].txn_id, 1);
}

#[tokio::test]
async fn synced_append_durable() {
    let dir = TempDir::new().unwrap();
    let seg = WalSegment::open(seg_path(&dir)).await.unwrap();
    let sink = Arc::new(WalSink::File(seg));
    let gc = WalGroupCommit::new(Arc::clone(&sink));

    gc.append(entry(1, 10).encode().unwrap(), WalDurability::Synced)
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
    let seg = WalSegment::open(seg_path(&dir)).await.unwrap();
    let sink = Arc::new(WalSink::File(seg));
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
            gc.append(entry(i, i * 10).encode().unwrap(), tier)
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
    let seg = WalSegment::open(seg_path(&dir)).await.unwrap();
    let sink = Arc::new(WalSink::File(seg));
    let gc = Arc::new(WalGroupCommit::new(Arc::clone(&sink)));

    let mut handles = Vec::new();
    for i in 1..=32u64 {
        let gc = Arc::clone(&gc);
        handles.push(tokio::spawn(async move {
            gc.append(entry(i, i * 10).encode().unwrap(), WalDurability::Synced)
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
    let seg = WalSegment::open(seg_path(&dir)).await.unwrap();
    let sink = Arc::new(WalSink::File(seg));
    let gc = Arc::new(WalGroupCommit::new(Arc::clone(&sink)));

    // A workload of ONLY Buffered appends — no matter how the windows
    // form, none of them carry a Synced waiter, so no fsync is ever
    // issued. fsync_count() == 0 is therefore deterministic, not timing
    // dependent.
    let mut handles = Vec::new();
    for i in 1..=16u64 {
        let gc = Arc::clone(&gc);
        handles.push(tokio::spawn(async move {
            gc.append(entry(i, i * 10).encode().unwrap(), WalDurability::Buffered)
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
async fn noop_append_returns_ok() {
    let sink = Arc::new(WalSink::Noop);
    let gc = WalGroupCommit::new(Arc::clone(&sink));

    gc.append(entry(1, 10).encode().unwrap(), WalDurability::Buffered)
        .await
        .unwrap();

    // Noop replay always returns empty.
    let replayed = sink.replay().await.unwrap();
    assert!(replayed.is_empty());
}

#[tokio::test]
async fn noop_sync_returns_ok() {
    let sink = Arc::new(WalSink::Noop);
    let gc = WalGroupCommit::new(Arc::clone(&sink));

    gc.append(entry(1, 10).encode().unwrap(), WalDurability::Synced)
        .await
        .unwrap();

    // Noop replay always returns empty.
    let replayed = sink.replay().await.unwrap();
    assert!(replayed.is_empty());
}
