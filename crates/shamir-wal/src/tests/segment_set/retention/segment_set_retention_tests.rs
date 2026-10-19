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
