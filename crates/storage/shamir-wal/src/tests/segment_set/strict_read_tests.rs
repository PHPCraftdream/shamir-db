use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;

use crate::{SegmentSet, WalEntryV2, WalStrictReadError};

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut bytes = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
    bytes
}

fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(dir)
        .unwrap()
        .map(|item| {
            let path = item.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}

async fn read_unchanged(dir: &Path) -> Result<Vec<WalEntryV2>, WalStrictReadError> {
    let before = snapshot(dir);
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        SegmentSet::read_entries_strict(dir),
    )
    .await
    .expect("strict WAL read timed out");
    assert_eq!(snapshot(dir), before);
    result
}

#[tokio::test]
async fn strict_read_uses_segment_and_frame_order_without_sidecars() {
    let dir = TempDir::new().unwrap();
    let make = |txn_id| frame(&WalEntryV2::new(txn_id, 7, vec![]).encode().unwrap());
    fs::write(dir.path().join("00000009.wal"), make(2)).unwrap();
    fs::write(dir.path().join("00000002.wal"), [make(3), make(1)].concat()).unwrap();
    fs::write(dir.path().join("00000002.meta"), b"not a sidecar").unwrap();

    let entries = read_unchanged(dir.path()).await.unwrap();
    let ids: Vec<_> = entries.into_iter().map(|entry| entry.txn_id).collect();
    assert_eq!(ids, [3, 1, 2]);
}

#[tokio::test]
async fn strict_read_rejects_partial_frames_in_any_segment() {
    let dir = TempDir::new().unwrap();
    let good = frame(&WalEntryV2::new(1, 7, vec![]).encode().unwrap());
    let offset = u64::try_from(good.len()).unwrap();
    let first = dir.path().join("00000000.wal");
    let last = dir.path().join("00000001.wal");
    crate::segment_meta::write_blocking(&first, 1, false).unwrap();
    for path in [&first, &last] {
        fs::write(&first, &good).unwrap();
        fs::write(&last, &good).unwrap();
        for cut in [3, 5, good.len() - 1] {
            fs::write(path, [&good[..], &good[..cut]].concat()).unwrap();
            assert!(matches!(
                read_unchanged(dir.path()).await,
                Err(WalStrictReadError::IncompleteFrame { path: found, offset: at })
                    if found == *path && at == offset
            ));
        }
    }
}

#[tokio::test]
async fn strict_read_checks_checksum_before_decode() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("00000000.wal");
    let mut invalid = frame(b"invalid entry");
    fs::write(&path, &invalid).unwrap();
    assert!(matches!(
        read_unchanged(dir.path()).await,
        Err(WalStrictReadError::Decode { path: found, offset: 0, .. }) if found == path
    ));

    *invalid.last_mut().unwrap() ^= 1;
    fs::write(&path, invalid).unwrap();
    assert!(matches!(
        read_unchanged(dir.path()).await,
        Err(WalStrictReadError::Other(error)) if error.to_string().contains("CRC mismatch")
    ));
}

#[tokio::test]
async fn strict_read_never_creates_missing_segments() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("missing");
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(10),
            SegmentSet::read_entries_strict(&missing)
        )
        .await
        .expect("strict WAL read timed out"),
        Err(WalStrictReadError::Other(_))
    ));
    assert!(!missing.exists());
    assert!(matches!(
        read_unchanged(dir.path()).await,
        Err(WalStrictReadError::Other(_))
    ));

    fs::write(dir.path().join("00000000.wal"), []).unwrap();
    fs::write(dir.path().join("0.wal"), []).unwrap();
    assert!(matches!(
        read_unchanged(dir.path()).await,
        Err(WalStrictReadError::Other(error)) if error.to_string().contains("non-canonical")
    ));
}
