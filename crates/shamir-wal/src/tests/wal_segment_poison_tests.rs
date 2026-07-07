use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;

use shamir_types::types::record_id::RecordId;
use tempfile::TempDir;

use crate::wal_entry_v2::{WalEntryV2, WalOpV2};
use crate::wal_segment::WalSegment;

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

/// Simulate a partial/torn write reaching the file (the ENOSPC-mid-write_all
/// scenario from audit §1.3): write two good frames through `append_batch`,
/// then *manually* extend the file with a torn tail (a len header promising
/// more bytes than are present) — exactly what a crashed `write_all` would
/// leave behind. A *fresh* `WalSegment` opened over that file must:
///
///   1. detect the torn tail on `repair_torn_tail` (or the next `append_batch`
///      via an internal repair), and
///   2. truncate the file back to the last good frame boundary so that
///      `replay()` cleanly returns the two intact frames.
///
/// Before the fix: the torn tail persisted on disk, so a later successful
/// `append_batch` window N+1 would land *behind* the torn frame and be
/// silently unreachable on replay (the exact data-loss scenario).
#[tokio::test]
async fn repair_torn_tail_restores_last_good_boundary() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);

    // Two good frames via the real append path.
    let seg = WalSegment::open(path.clone()).await.unwrap();
    let entries = [entry(1, 10), entry(2, 20)];
    let payloads: Vec<Vec<u8>> = entries.iter().map(|e| e.encode().unwrap()).collect();
    seg.append_batch(Arc::new(payloads), 20).await.unwrap();
    let good_len = std::fs::metadata(&path).unwrap().len();

    // Append a torn tail: a len header claiming 999 bytes, then only 2 bytes.
    {
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&999u32.to_le_bytes()).unwrap();
        f.write_all(b"xx").unwrap();
        f.flush().unwrap();
    }
    let torn_len = std::fs::metadata(&path).unwrap().len();
    assert!(torn_len > good_len, "torn tail must extend the file");

    // Re-open (simulating the in-process continuation after the failed write).
    // `repair_torn_tail` removes the partial frame from the file.
    let seg2 = WalSegment::open(path.clone()).await.unwrap();
    seg2.repair_torn_tail().await.unwrap();

    let repaired_len = std::fs::metadata(&path).unwrap().len();
    assert_eq!(
        repaired_len, good_len,
        "torn tail must be truncated back to the last good boundary"
    );

    let replayed = seg2.replay().await.unwrap();
    assert_eq!(replayed.len(), 2, "the two good frames survive");
    assert_eq!(replayed[0].txn_id, 1);
    assert_eq!(replayed[1].txn_id, 2);
}

/// Once a segment has seen a write/sync failure, further `append_batch` calls
/// must refuse — the file is quarantined and the leader must rotate to a new
/// segment instead of writing more data behind a potentially-torn tail.
#[tokio::test]
async fn poisoned_segment_rejects_further_appends() {
    let dir = TempDir::new().unwrap();
    let seg = WalSegment::open(seg_path(&dir)).await.unwrap();

    // Sanity: appends work before poisoning.
    seg.append_batch(Arc::new(vec![entry(1, 10).encode().unwrap()]), 10)
        .await
        .unwrap();
    assert!(!seg.is_poisoned());

    // Mark poisoned (as a real write/sync failure would internally).
    seg.mark_poisoned();
    assert!(seg.is_poisoned());

    // A subsequent append must fail fast — never touch the bad file again.
    let res = seg
        .append_batch(Arc::new(vec![entry(2, 20).encode().unwrap()]), 20)
        .await;
    assert!(
        res.is_err(),
        "append_batch on a poisoned segment must error, got {:?}",
        res
    );
    let msg = res.unwrap_err().to_string();
    assert!(
        msg.contains("poisoned") || msg.contains("Poisoned"),
        "error should mention the segment is poisoned, got: {}",
        msg
    );
}

/// `repair_torn_tail` on an already-intact file is a no-op (the common case:
/// every clean open finds no torn tail to repair).
#[tokio::test]
async fn repair_torn_tail_noop_on_clean_file() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();

    seg.append_batch(Arc::new(vec![entry(1, 10).encode().unwrap()]), 10)
        .await
        .unwrap();
    let before = std::fs::metadata(&path).unwrap().len();

    seg.repair_torn_tail().await.unwrap();
    let after = std::fs::metadata(&path).unwrap().len();
    assert_eq!(before, after, "clean file must be untouched");

    let replayed = seg.replay().await.unwrap();
    assert_eq!(replayed.len(), 1);
}
