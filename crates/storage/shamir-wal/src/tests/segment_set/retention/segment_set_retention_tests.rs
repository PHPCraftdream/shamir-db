use shamir_types::types::record_id::RecordId;
use tempfile::TempDir;

use crate::segment_set::SegmentSet;
use crate::wal_entry_v2::{WalEntryV2, WalOpV2};

fn entry(version: u64) -> WalEntryV2 {
    let mut id = [0; 16];
    id[15] = version as u8;
    WalEntryV2::new(
        version,
        1,
        vec![WalOpV2::Delete {
            table_id_interned: 1,
            rid: RecordId(id),
        }],
    )
    .with_commit_version(version)
}

#[tokio::test]
async fn rv_wal_8021eb80_mixed_sealed_sidecar_and_legacy_fallback() {
    for legacy in [false, true] {
        let dir = TempDir::new().unwrap();
        let segments = SegmentSet::open(dir.path().to_path_buf(), 1).await.unwrap();
        segments
            .append_batch(
                vec![entry(0).encode().unwrap(), entry(10).encode().unwrap()],
                10,
            )
            .await
            .unwrap();
        segments
            .append_batch(vec![entry(20).encode().unwrap()], 20)
            .await
            .unwrap();
        assert_eq!(segments.max_committed(), 20);
        assert_eq!(segments.truncate_below(20).await.unwrap(), 1);
        assert_eq!(segments.max_committed(), 10);
        drop(segments);
        if legacy {
            let mut bytes = [0u8; 17];
            bytes[..4].copy_from_slice(b"WMT2");
            bytes[4] = 2;
            bytes[5..13].copy_from_slice(&10u64.to_le_bytes());
            let crc = crc32fast::hash(&bytes[..13]);
            bytes[13..].copy_from_slice(&crc.to_le_bytes());
            std::fs::write(dir.path().join("00000000.meta"), bytes).unwrap();
        }
        let reopened = SegmentSet::open(dir.path().to_path_buf(), 1).await.unwrap();
        assert_eq!(reopened.max_committed(), 10);
        assert!(!reopened.has_truncatable(u64::MAX));
        assert_eq!(reopened.truncate_below(u64::MAX).await.unwrap(), 0);
        assert_eq!(
            reopened
                .replay()
                .await
                .unwrap()
                .iter()
                .map(|e| e.commit_version)
                .collect::<Vec<_>>(),
            vec![0, 10]
        );
    }
}

#[tokio::test]
async fn rv_wal_8021eb80_active_reopen_then_poison_rotation_keeps_pin() {
    let dir = TempDir::new().unwrap();
    let segments = SegmentSet::open(dir.path().to_path_buf(), u64::MAX)
        .await
        .unwrap();
    segments
        .append_batch(vec![entry(0).encode().unwrap()], 0)
        .await
        .unwrap();
    drop(segments);
    let segments = SegmentSet::open(dir.path().to_path_buf(), u64::MAX)
        .await
        .unwrap();
    segments
        .append_batch(vec![entry(10).encode().unwrap()], 10)
        .await
        .unwrap();
    segments.active_segment_for_test().mark_poisoned();
    segments
        .append_batch(vec![entry(20).encode().unwrap()], 20)
        .await
        .unwrap();
    assert!(!segments.has_truncatable(u64::MAX));
    assert_eq!(segments.truncate_below(u64::MAX).await.unwrap(), 0);
    drop(segments);
    let reopened = SegmentSet::open(dir.path().to_path_buf(), u64::MAX)
        .await
        .unwrap();
    assert_eq!(reopened.max_committed(), 20);
    assert!(!reopened.has_truncatable(u64::MAX));
    assert_eq!(reopened.truncate_below(u64::MAX).await.unwrap(), 0);
    assert_eq!(
        reopened
            .replay()
            .await
            .unwrap()
            .iter()
            .map(|e| e.commit_version)
            .collect::<Vec<_>>(),
        vec![0, 10, 20]
    );
}

#[tokio::test]
async fn rv_wal_8021eb80_wmt2_highest_seal_is_scanned_and_never_reactivated() {
    let dir = TempDir::new().unwrap();
    let segments = SegmentSet::open(dir.path().to_path_buf(), u64::MAX)
        .await
        .unwrap();
    segments
        .append_batch(
            vec![entry(0).encode().unwrap(), entry(10).encode().unwrap()],
            10,
        )
        .await
        .unwrap();
    segments.sync().await.unwrap();
    drop(segments);
    let mut bytes = [0u8; 17];
    bytes[..4].copy_from_slice(b"WMT2");
    bytes[4] = 2;
    bytes[5..13].copy_from_slice(&10u64.to_le_bytes());
    let crc = crc32fast::hash(&bytes[..13]);
    bytes[13..].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(dir.path().join("00000000.meta"), bytes).unwrap();
    let reopened = SegmentSet::open(dir.path().to_path_buf(), u64::MAX)
        .await
        .unwrap();
    assert!(dir.path().join("00000001.wal").exists());
    assert_eq!(reopened.max_committed(), 10);
    assert!(!reopened.has_truncatable(10));
    assert_eq!(reopened.truncate_below(10).await.unwrap(), 0);
    assert_eq!(reopened.replay().await.unwrap().len(), 2);
}

#[tokio::test]
async fn retained_bytes_drop_to_zero_after_truncating_versioned_sealed_files() {
    let dir = TempDir::new().unwrap();
    let segments = SegmentSet::open(dir.path().to_path_buf(), 64)
        .await
        .unwrap();
    for version in 1..=12 {
        segments
            .append_batch(vec![entry(version).encode().unwrap()], version)
            .await
            .unwrap();
    }
    let sealed_bytes = segments.sealed_retained_bytes().await.unwrap();
    assert!(sealed_bytes > 0);
    let mut wal_files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wal"))
        .collect();
    wal_files.sort_by_key(|path| {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u64>().ok())
            .expect("numeric WAL segment filename")
    });
    let active = wal_files.pop().expect("active WAL segment");
    assert!(!wal_files.is_empty(), "test must include sealed segments");
    let removed = segments.truncate_below(u64::MAX).await.unwrap();
    assert!(removed > 0);
    assert_eq!(segments.sealed_retained_bytes().await.unwrap(), 0);
    let remaining_wal_count = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "wal"))
        .count();
    assert_eq!(
        remaining_wal_count, 1,
        "only the active WAL segment remains"
    );
    assert!(
        active.exists(),
        "truncation must leave the active WAL segment intact"
    );
}
