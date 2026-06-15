use std::fs::OpenOptions;
use std::io::Write;

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

#[tokio::test]
async fn append_then_replay_roundtrips() {
    let dir = TempDir::new().unwrap();
    let seg = WalSegment::open(seg_path(&dir)).await.unwrap();

    let entries = [entry(1, 10), entry(2, 20), entry(3, 30)];
    let payloads: Vec<Vec<u8>> = entries.iter().map(|e| e.encode().unwrap()).collect();

    let last_seq = seg.append_batch(payloads, 30).await.unwrap();
    assert_eq!(last_seq, 2); // seqs 0,1,2
    assert_eq!(seg.max_committed(), 30);

    let replayed = seg.replay().await.unwrap();
    assert_eq!(replayed.len(), 3);
    for (got, want) in replayed.iter().zip(entries.iter()) {
        assert_eq!(got.txn_id, want.txn_id);
        assert_eq!(got.commit_version, want.commit_version);
    }
}

#[tokio::test]
async fn replay_stops_at_torn_tail() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();

    let entries = [entry(1, 10), entry(2, 20)];
    let payloads: Vec<Vec<u8>> = entries.iter().map(|e| e.encode().unwrap()).collect();
    seg.append_batch(payloads, 20).await.unwrap();

    // Append a torn frame: a len header claiming 999 bytes follow, but
    // only a couple of bytes are actually written.
    {
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&999u32.to_le_bytes()).unwrap();
        f.write_all(b"xx").unwrap();
        f.flush().unwrap();
    }

    let replayed = seg.replay().await.unwrap();
    assert_eq!(replayed.len(), 2);
    assert_eq!(replayed[0].txn_id, 1);
    assert_eq!(replayed[1].txn_id, 2);
}

#[tokio::test]
async fn crc_detects_corruption() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();

    let e = entry(1, 10);
    seg.append_batch(vec![e.encode().unwrap()], 10)
        .await
        .unwrap();

    // Flip a byte inside the payload region (offset 4 + something: skip
    // the len header at [0..4], the magic at [4..8], hit version/body).
    let mut bytes = std::fs::read(&path).unwrap();
    let flip = 9; // payload byte (past 4-byte len + 4-byte magic + version)
    bytes[flip] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();

    let replayed = seg.replay().await.unwrap();
    assert_eq!(replayed.len(), 0);
}

#[tokio::test]
async fn corruption_in_first_frame_stops_replay_entirely() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();

    let entries = [entry(1, 10), entry(2, 20)];
    let payloads: Vec<Vec<u8>> = entries.iter().map(|e| e.encode().unwrap()).collect();
    seg.append_batch(payloads, 20).await.unwrap();

    // Corrupt the FIRST frame's payload (flip a byte past the 4-byte len header).
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[6] ^= 0xFF; // inside first payload
    std::fs::write(&path, &bytes).unwrap();

    let replayed = seg.replay().await.unwrap();
    // First frame CRC fails → replay returns 0; the valid second frame is NOT recovered.
    assert_eq!(
        replayed.len(),
        0,
        "corruption in first frame must stop replay; second frame must NOT be recovered"
    );
}

#[tokio::test]
async fn sync_after_append_succeeds() {
    let dir = TempDir::new().unwrap();
    let seg = WalSegment::open(seg_path(&dir)).await.unwrap();

    seg.append_batch(vec![entry(1, 10).encode().unwrap()], 10)
        .await
        .unwrap();
    seg.sync().await.unwrap();

    let replayed = seg.replay().await.unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].txn_id, 1);
}

#[tokio::test]
async fn empty_segment_replays_empty() {
    let dir = TempDir::new().unwrap();
    let seg = WalSegment::open(seg_path(&dir)).await.unwrap();
    let replayed = seg.replay().await.unwrap();
    assert!(replayed.is_empty());
}
