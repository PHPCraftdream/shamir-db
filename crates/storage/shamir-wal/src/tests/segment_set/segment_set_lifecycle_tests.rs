use std::sync::Arc;

use shamir_types::types::record_id::RecordId;
use tempfile::TempDir;

use crate::segment_set::SegmentSet;
use crate::wal_entry_v2::{WalEntryV2, WalOpV2};

fn entry(txn_id: u64, commit_version: u64) -> WalEntryV2 {
    let mut rid = [0u8; 16];
    rid[15] = txn_id as u8;
    WalEntryV2::new(
        txn_id,
        7,
        vec![WalOpV2::Delete {
            table_id_interned: 7,
            rid: RecordId(rid),
        }],
    )
    .with_commit_version(commit_version)
}

#[tokio::test]
async fn rv_wal_67475c86_middle_failure_preserves_retry_and_later_progress() {
    let dir = TempDir::new().unwrap();
    let set = SegmentSet::open(dir.path().to_path_buf(), 1).await.unwrap();
    for version in 1..=3 {
        set.append_batch(vec![entry(version, version).encode().unwrap()], version)
            .await
            .unwrap();
    }
    set.inject_truncate_failure(1);
    let error = set.truncate_below(3).await.unwrap_err();
    let shamir_storage::error::DbError::Io(ref io) = error else {
        panic!("expected typed I/O error: {error}");
    };
    assert_eq!(io.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("2 deleted, 1 failed"));
    assert!(!dir.path().join("00000000.wal").exists());
    assert!(dir.path().join("00000001.wal").exists());
    assert!(dir.path().join("00000001.meta").exists());
    assert!(!dir.path().join("00000002.wal").exists());
    assert!(set.has_truncatable(3));
    assert_eq!(set.replay().await.unwrap()[0].commit_version, 2);
    assert_eq!(set.truncate_below(3).await.unwrap(), 1);
    assert!(!set.has_truncatable(3));
    assert!(set.replay().await.unwrap().is_empty());
}

#[tokio::test]
async fn rv_wal_67475c86_cancelled_unlink_reconciles_before_next_truncator() {
    let dir = TempDir::new().unwrap();
    let set = Arc::new(SegmentSet::open(dir.path().to_path_buf(), 1).await.unwrap());
    for version in 1..=3 {
        set.append_batch(vec![entry(version, version).encode().unwrap()], version)
            .await
            .unwrap();
    }
    set.inject_truncate_failure(1);
    let (entered, resume) = set.pause_next_truncate();
    let worker = Arc::clone(&set);
    let task = tokio::spawn(async move { worker.truncate_below(3).await });
    entered.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let worker = Arc::clone(&set);
    let retry = tokio::spawn(async move { worker.truncate_below(3).await });
    resume.send(()).unwrap();
    assert_eq!(retry.await.unwrap().unwrap(), 1);
    assert!(!set.has_truncatable(3));
    assert!(set.replay().await.unwrap().is_empty());
}

#[tokio::test]
async fn retained_bytes_waits_for_cancelled_truncation() {
    let dir = TempDir::new().unwrap();
    let set = Arc::new(SegmentSet::open(dir.path().to_path_buf(), 1).await.unwrap());
    set.append_batch(vec![entry(1, 1).encode().unwrap()], 1)
        .await
        .unwrap();
    let (entered, resume) = set.pause_next_truncate();
    let worker = Arc::clone(&set);
    let task = tokio::spawn(async move { worker.truncate_below(1).await });
    entered.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let worker = Arc::clone(&set);
    let measurement = tokio::spawn(async move { worker.sealed_retained_bytes().await });
    tokio::task::yield_now().await;
    assert!(!measurement.is_finished());
    resume.send(()).unwrap();
    assert_eq!(measurement.await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn retained_bytes_reports_unexpected_missing_segment() {
    let dir = TempDir::new().unwrap();
    let set = SegmentSet::open(dir.path().to_path_buf(), 1).await.unwrap();
    set.append_batch(vec![entry(1, 1).encode().unwrap()], 1)
        .await
        .unwrap();
    std::fs::remove_file(dir.path().join("00000000.wal")).unwrap();
    assert!(set.sealed_retained_bytes().await.is_err());
}

#[tokio::test]
async fn rv_wal_ae1ca19c_rejects_noncanonical_names_and_sequence_aliases() {
    // A bare non-canonical numeric `.wal` name is rejected outright.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("5.wal"), []).unwrap();
    let result = SegmentSet::open(dir.path().to_path_buf(), 1 << 20).await;
    assert!(
        result.is_err(),
        "non-canonical numeric segment names must be rejected: {:?}",
        result.map(|_| ()).map_err(|e| e.to_string())
    );

    // Genuinely non-segment filenames are ignored.
    let other_dir = TempDir::new().unwrap();
    std::fs::write(other_dir.path().join("not-a-segment.txt"), []).unwrap();
    assert!(
        SegmentSet::open(other_dir.path().to_path_buf(), 1 << 20)
            .await
            .is_ok(),
        "non-segment filenames are ignored"
    );

    let alias_dir = TempDir::new().unwrap();
    std::fs::write(alias_dir.path().join("00000005.wal"), []).unwrap();
    std::fs::write(alias_dir.path().join("5.wal"), []).unwrap();
    let result = SegmentSet::open(alias_dir.path().to_path_buf(), 1 << 20).await;
    assert!(
        result.is_err(),
        "sequence aliases must not shadow WAL files"
    );
}

#[tokio::test]
async fn rv_wal_48749f97_restores_active_commit_watermark_on_reopen() {
    let dir = TempDir::new().unwrap();
    let set = SegmentSet::open(dir.path().to_path_buf(), 1 << 20)
        .await
        .unwrap();
    for version in [12, 37, 24] {
        set.append_batch(vec![entry(version, version).encode().unwrap()], version)
            .await
            .unwrap();
    }
    drop(set);

    let reopened = SegmentSet::open(dir.path().to_path_buf(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(reopened.max_committed(), 37);
}

#[tokio::test]
async fn rv_wal_8077da84_poison_rotation_failure_recovers_on_retry() {
    let dir = TempDir::new().unwrap();
    let set = SegmentSet::open(dir.path().to_path_buf(), 1 << 20)
        .await
        .unwrap();
    set.append_batch(vec![entry(1, 10).encode().unwrap()], 10)
        .await
        .unwrap();
    set.active_segment_for_test().mark_poisoned();
    let next_path = dir.path().join("00000001.wal");
    std::fs::create_dir(&next_path).unwrap();

    let failed = set
        .append_batch(vec![entry(2, 20).encode().unwrap()], 20)
        .await;
    assert!(
        failed.is_err(),
        "opening a directory as the next WAL must fail"
    );
    assert_eq!(set.max_committed(), 10);
    assert!(set.active_segment_for_test().is_poisoned());

    std::fs::remove_dir(next_path).unwrap();
    set.append_batch(vec![entry(3, 30).encode().unwrap()], 30)
        .await
        .unwrap();
    assert_eq!(
        set.replay()
            .await
            .unwrap()
            .iter()
            .map(|entry| entry.txn_id)
            .collect::<Vec<_>>(),
        vec![1, 3]
    );
}
