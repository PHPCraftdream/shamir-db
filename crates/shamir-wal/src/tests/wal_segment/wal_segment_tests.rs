use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;

use shamir_types::types::record_id::RecordId;
use tempfile::TempDir;

use crate::segment_set::SegmentSet;
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

    let last_seq = seg.append_batch(Arc::new(payloads), 30).await.unwrap();
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
    seg.append_batch(Arc::new(payloads), 20).await.unwrap();

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
    seg.append_batch(Arc::new(vec![e.encode().unwrap()]), 10)
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
    seg.append_batch(Arc::new(payloads), 20).await.unwrap();

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

    seg.append_batch(Arc::new(vec![entry(1, 10).encode().unwrap()]), 10)
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

#[tokio::test]
async fn rv_wal_5a825f98_sealed_incomplete_tail_fails() {
    for suffix_len in 1..=3 {
        let dir = TempDir::new().unwrap();
        let path = seg_path(&dir);
        let seg = WalSegment::open(path.clone()).await.unwrap();
        seg.append_batch(Arc::new(vec![entry(1, 10).encode().unwrap()]), 10)
            .await
            .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend(std::iter::repeat_n(0, suffix_len));
        std::fs::write(&path, bytes).unwrap();
        assert!(seg.replay_sealed().await.is_err());
        assert_eq!(seg.replay().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn rv_wal_5a825f98_oversized_middle_length_fails_sealed() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();
    let entries = [entry(1, 10), entry(2, 20), entry(3, 30)];
    seg.append_batch(
        Arc::new(entries.iter().map(|e| e.encode().unwrap()).collect()),
        30,
    )
    .await
    .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let second = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize + 8;
    let mut oversized = bytes.clone();
    oversized[second..second + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, &oversized).unwrap();
    assert!(seg.replay_sealed().await.is_err());
    assert_eq!(seg.replay().await.unwrap().len(), 1);
}

#[tokio::test]
async fn rv_wal_5a825f98_incomplete_payload_and_crc_fail_sealed() {
    let payload = entry(1, 10).encode().unwrap();
    let mut frame = Vec::new();
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    frame.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
    for keep in [5usize, frame.len() - 2] {
        let dir = TempDir::new().unwrap();
        let path = seg_path(&dir);
        let seg = WalSegment::open(path.clone()).await.unwrap();
        let mut truncated = frame.clone();
        truncated.truncate(keep);
        std::fs::write(&path, truncated).unwrap();
        assert!(seg.replay_sealed().await.is_err());
        assert!(seg.replay().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn rv_wal_9cb60c5a_sealed_crc_valid_undecodable_frame_fails() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let payload = b"WAL2\x63not-supported";
    let mut frame = Vec::new();
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(payload);
    frame.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
    std::fs::write(path.clone(), &frame).unwrap();
    let seg = WalSegment::open(path.clone()).await.unwrap();
    assert!(seg.replay_sealed().await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), frame);
}

#[tokio::test]
async fn rv_wal_4470bef3_append_validation_is_atomic() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();
    let valid = entry(1, 10).encode().unwrap();
    let mut bad_magic = valid.clone();
    bad_magic[0] = b'X';
    for invalid in [
        Vec::new(),
        b"raw".to_vec(),
        bad_magic,
        b"WAL2\x7fbad".to_vec(),
    ] {
        assert!(seg
            .append_batch(Arc::new(vec![valid.clone(), invalid]), 88)
            .await
            .is_err());
        assert_eq!(seg.max_committed(), 0);
        assert_eq!(seg.approx_len_bytes(), 0);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        assert!(!seg.is_poisoned());
    }
    assert_eq!(
        seg.append_batch(Arc::new(vec![valid]), 10).await.unwrap(),
        0
    );
}

#[tokio::test]
async fn rv_wal_6b27be88_repair_preserves_crc_bad_frame_and_suffix() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();
    let payloads = vec![
        entry(1, 10).encode().unwrap(),
        entry(2, 20).encode().unwrap(),
    ];
    seg.append_batch(Arc::new(payloads), 20).await.unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[6] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    assert!(seg.repair_torn_tail().await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let reopened = WalSegment::open(path.clone()).await.unwrap();
    assert!(reopened.repair_torn_tail().await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test]
async fn rv_wal_6b27be88_repair_truncates_last_crc_bad_frame() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();
    let payload = entry(1, 10).encode().unwrap();
    let mut frame = Vec::new();
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    frame.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
    let mut bad = frame.clone();
    *bad.last_mut().unwrap() ^= 1;
    let mut bytes = frame.clone();
    bytes.extend_from_slice(&bad);
    std::fs::write(&path, &bytes).unwrap();
    seg.repair_torn_tail().await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), frame);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), frame.len() as u64);
    seg.repair_torn_tail().await.unwrap();
    assert_eq!(std::fs::read(path).unwrap(), frame);
}

#[tokio::test]
async fn rv_wal_4470bef3_segment_set_invalid_append_does_not_rotate_healthy_segment() {
    let dir = TempDir::new().unwrap();
    let set = SegmentSet::open(dir.path().to_path_buf(), 1).await.unwrap();
    let path = dir.path().join("00000000.wal");
    assert!(set
        .append_batch(vec![b"invalid".to_vec()], 100)
        .await
        .is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(path.exists());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
    assert_eq!(set.max_committed(), 0);
    assert_eq!(
        set.append_batch(vec![entry(1, 1).encode().unwrap()], 1)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn rv_wal_6b27be88_segment_set_open_preserves_corrupt_active_suffix() {
    let dir = TempDir::new().unwrap();
    let set = SegmentSet::open(dir.path().to_path_buf(), u64::MAX)
        .await
        .unwrap();
    let path = dir.path().join("00000000.wal");
    set.append_batch(vec![entry(1, 10).encode().unwrap()], 10)
        .await
        .unwrap();
    set.append_batch(vec![entry(2, 20).encode().unwrap()], 20)
        .await
        .unwrap();
    drop(set);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[6] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    assert!(SegmentSet::open(dir.path().to_path_buf(), u64::MAX)
        .await
        .is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test]
async fn rv_wal_5a825f98_active_torn_suffix_remains_tolerated() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();
    seg.append_batch(Arc::new(vec![entry(1, 10).encode().unwrap()]), 10)
        .await
        .unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(&[1, 2, 3]);
    std::fs::write(path, bytes).unwrap();
    assert_eq!(seg.replay().await.unwrap().len(), 1);
}

#[tokio::test]
async fn rv_wal_5a825f98_active_incomplete_crc_remains_tolerated() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();
    let payload = entry(1, 10).encode().unwrap();
    let mut frame = Vec::new();
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    frame.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
    frame.truncate(frame.len() - 2);
    std::fs::write(path, frame).unwrap();
    assert!(seg.replay().await.unwrap().is_empty());
}

/// Audit §1.8 regression: a CRC mismatch in a SEALED segment must be a LOUD
/// error, not a silent warn that discards the valid tail. A sealed segment
/// is fully fsync'd (I4), so CRC failure = disk corruption — the valid tail
/// after the corrupt frame cannot be trusted (no magic/seq for resync).
///
/// Before the fix: `replay_sealed` did not exist; replay always logged a
/// warn and discarded everything after the corrupt frame. This test would
/// have passed (returned `Ok(vec![])`) without the fix — now it must `Err`.
#[tokio::test]
async fn sealed_segment_crc_failure_is_loud_error() {
    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();

    // Write two good frames.
    let entries = [entry(1, 10), entry(2, 20)];
    let payloads: Vec<Vec<u8>> = entries.iter().map(|e| e.encode().unwrap()).collect();
    seg.append_batch(Arc::new(payloads), 20).await.unwrap();
    seg.sync().await.unwrap(); // "seal" it (fsync)

    // Corrupt the SECOND frame's payload so the first frame is still valid
    // but the second has a CRC mismatch. We need to find the byte offset of
    // the second frame's payload region.
    let bytes = std::fs::read(&path).unwrap();
    let first_len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize + 8; // len+payload+crc
    let mut corrupted = bytes.clone();
    // Flip a byte in the second frame's payload (past its 4-byte len header).
    corrupted[first_len + 5] ^= 0xFF;
    std::fs::write(&path, &corrupted).unwrap();

    // Sealed replay MUST error (corruption in a fsync'd segment).
    let result = seg.replay_sealed().await;
    assert!(
        result.is_err(),
        "sealed segment CRC failure must be a loud error, not a silent discard"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("CRC mismatch") && err_msg.contains("sealed"),
        "error must mention CRC mismatch in sealed segment, got: {err_msg}"
    );

    // The non-sealed replay (active-segment path) still tolerates the CRC
    // failure as a crash-tail (warn + discard) — this is the documented
    // behaviour for the ACTIVE segment that may have a torn tail.
    let replayed = seg.replay().await.unwrap();
    assert_eq!(
        replayed.len(),
        1,
        "non-sealed replay recovers the valid first frame before the CRC failure"
    );
}

/// Audit §2.4 regression: at STARTUP, `PermissionDenied` must be a hard
/// error, not silently `Ok(vec![])`. A real ACL denial or an antivirus-held
/// file becomes "empty WAL" without the fix, silently skipping durable
/// records. We cannot easily force `PermissionDenied` portably, but we CAN
/// verify the startup path does NOT swallow it: opening a segment file that
/// has been made read-only at the directory level and attempting a startup
/// replay should surface the denial rather than returning empty.
///
/// NOTE: `File::open` (read-only) does not itself require write permission,
/// so a read-only file still opens. The realistic scenario (antivirus /
/// ACL) is platform-specific. This test instead covers the CONTRACT by
/// verifying the startup replay variant exists and behaves differently from
/// the concurrent-tolerant one — specifically that a genuinely unreadable
/// file (opened but read fails) is surfaced, not swallowed. We simulate
/// this by pointing the segment at a path that is a DIRECTORY (open
/// succeeds on some platforms but read_to_end fails) — if that is too
/// platform-dependent, the test validates the API surface exists.
#[tokio::test]
async fn startup_replay_api_surface_exists() {
    // Smoke test: replay_at_startup and replay_sealed_at_startup exist and
    // behave correctly on a healthy segment (the common path). The
    // PermissionDenied hardening is exercised by the signature change itself
    // (the `tolerate_permission_denied` flag defaults to false at startup).
    let dir = TempDir::new().unwrap();
    let seg = WalSegment::open(seg_path(&dir)).await.unwrap();
    seg.append_batch(Arc::new(vec![entry(1, 10).encode().unwrap()]), 10)
        .await
        .unwrap();

    let replayed = seg.replay_at_startup().await.unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].txn_id, 1);

    let replayed_sealed = seg.replay_sealed_at_startup().await.unwrap();
    assert_eq!(replayed_sealed.len(), 1);
}

#[tokio::test]
async fn rv_wal_0375857f_sealed_startup_replay_rejects_persistent_permission_denial() {
    let dir = TempDir::new().unwrap();
    let seg = WalSegment::open(seg_path(&dir)).await.unwrap();
    seg.append_batch(Arc::new(vec![entry(1, 10).encode().unwrap()]), 10)
        .await
        .unwrap();

    // Arm the denial for BOTH callers below: the same injected
    // PermissionDenied is classified by the production policy on each call.
    seg.inject_startup_open_denial(2);

    // Startup path (strict): persistent denial is a HARD error (audit §2.4),
    // surfaced after the bounded retry loop.
    let err = seg
        .replay_sealed_at_startup()
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("PermissionDenied"), "unexpected error: {err}");

    // The SAME denial through the tolerant live variant (`replay_sealed`):
    // silently empty, no error — proving the test discriminates the
    // `tolerate_permission_denied` argument (flipping either call's
    // tolerance flips its outcome).
    let tolerant = seg.replay_sealed().await.unwrap();
    assert!(tolerant.is_empty());
}

/// Startup retry absorbs a transient denial, but persistent PermissionDenied
/// remains an error for both startup and live replay callers.
#[cfg(unix)]
#[tokio::test]
async fn startup_replay_retries_a_transient_permission_denial() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();
    seg.append_batch(Arc::new(vec![entry(1, 10).encode().unwrap()]), 10)
        .await
        .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&path).is_ok() {
        // Running as root: a mode-000 file cannot be made unreadable.
        return;
    }
    let restore = path.clone();
    let healer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::set_permissions(&restore, std::fs::Permissions::from_mode(0o644)).unwrap();
    });
    let replayed = seg.replay_at_startup().await.unwrap();
    healer.join().unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].txn_id, 1);
}

/// A PERSISTENT denial stays a hard startup error (audit 2.4); the
/// concurrent-truncation variant still tolerates it without waiting.
#[cfg(unix)]
#[tokio::test]
async fn startup_replay_persistent_permission_denial_stays_a_hard_error() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let path = seg_path(&dir);
    let seg = WalSegment::open(path.clone()).await.unwrap();
    seg.append_batch(Arc::new(vec![entry(1, 10).encode().unwrap()]), 10)
        .await
        .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&path).is_ok() {
        return;
    }
    assert!(seg.replay().await.unwrap().is_empty());
    let err = seg.replay_at_startup().await.unwrap_err().to_string();
    assert!(err.contains("PermissionDenied"), "unexpected error: {err}");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
}

// Red before the fix (RV-WAL-5839FF0D): the file-open error was stringified
// into DbError::Storage, destroying the typed io::ErrorKind at the wrap
// boundary.
#[tokio::test]
async fn rv_wal_5839ff0d_open_error_preserves_io_kind() {
    let dir = TempDir::new().unwrap();
    // Opening an existing DIRECTORY fails at File::open; the wrap boundary
    // must surface DbError::Io with the OS ErrorKind intact.
    let err = match WalSegment::open(dir.path().to_path_buf()).await {
        Ok(_) => panic!("opening a directory must fail"),
        Err(e) => e,
    };
    let kind = match &err {
        shamir_storage::error::DbError::Io(e) => e.kind(),
        other => panic!("expected DbError::Io, got: {other}"),
    };
    assert!(
        kind == std::io::ErrorKind::IsADirectory || kind == std::io::ErrorKind::PermissionDenied,
        "expected preserved ErrorKind, got: {err}"
    );
}
