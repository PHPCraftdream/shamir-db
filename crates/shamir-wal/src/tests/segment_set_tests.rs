use std::sync::Arc;

use shamir_types::types::record_id::RecordId;
use tempfile::TempDir;

use crate::segment_set::SegmentSet;
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

/// Count `*.wal` segment files currently on disk in `dir`.
fn seg_file_count(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.ends_with(".wal"))
                .unwrap_or(false)
        })
        .count()
}

#[tokio::test]
async fn rotation_on_size() {
    let dir = TempDir::new().unwrap();
    // Tiny max_bytes so a couple of appends force several rotations.
    let set = SegmentSet::open(dir.path().to_path_buf(), 64)
        .await
        .unwrap();

    for i in 1..=10u64 {
        set.append_batch(vec![entry(i, i).encode().unwrap()], i)
            .await
            .unwrap();
    }

    let files = seg_file_count(dir.path());
    assert!(
        files > 1,
        "expected rotation to produce >1 segment file, got {files}"
    );
}

#[tokio::test]
async fn replay_across_segments_is_identical() {
    let entries: Vec<WalEntryV2> = (1..=12u64).map(|i| entry(i, i)).collect();

    // (a) Segmented set with a tiny threshold → many rotations.
    let seg_dir = TempDir::new().unwrap();
    let set = SegmentSet::open(seg_dir.path().to_path_buf(), 48)
        .await
        .unwrap();
    for e in &entries {
        set.append_batch(vec![e.encode().unwrap()], e.commit_version)
            .await
            .unwrap();
    }
    let set_replay = set.replay().await.unwrap();

    // (b) Single segment with the exact same data in the exact same order.
    let single_dir = TempDir::new().unwrap();
    let single = WalSegment::open(single_dir.path().join("single.wal"))
        .await
        .unwrap();
    for e in &entries {
        single
            .append_batch(vec![e.encode().unwrap()], e.commit_version)
            .await
            .unwrap();
    }
    let single_replay = single.replay().await.unwrap();

    assert_eq!(set_replay.len(), entries.len());
    assert_eq!(
        set_replay, single_replay,
        "segmented replay must be byte-identical to a single segment"
    );
    // And identical to the source order.
    assert_eq!(set_replay, entries);
}

#[tokio::test]
async fn truncate_drops_drained_keeps_undrained() {
    let dir = TempDir::new().unwrap();
    // Small threshold so each append seals quickly and we get several sealed.
    let set = SegmentSet::open(dir.path().to_path_buf(), 32)
        .await
        .unwrap();

    // Versions strictly increasing 10,20,...,80. Each append rotates.
    for k in 1..=8u64 {
        let v = k * 10;
        set.append_batch(vec![entry(k, v).encode().unwrap()], v)
            .await
            .unwrap();
    }

    let before = set.replay().await.unwrap();
    assert_eq!(before.len(), 8);

    // Truncate below 45 → sealed segments with max_version 10,20,30,40 go;
    // 50,60,70 sealed survive, 80 is in the active segment.
    let deleted = set.truncate_below(45).await.unwrap();
    assert!(deleted >= 1, "expected at least one sealed segment dropped");

    let after = set.replay().await.unwrap();
    let versions: Vec<u64> = after.iter().map(|e| e.commit_version).collect();
    // Every surviving record has version > 45 (the drained ones are gone).
    assert!(
        versions.iter().all(|&v| v > 45),
        "drained (v<=45) records must be gone, got {versions:?}"
    );
    // The undrained tail is intact and ordered.
    assert_eq!(versions, vec![50, 60, 70, 80]);
}

#[tokio::test]
async fn truncate_never_drops_active() {
    let dir = TempDir::new().unwrap();
    // Large threshold → NO rotation; every record lives in the single active
    // segment. truncate_below(MAX) must therefore drop NOTHING (the active
    // segment is never deletable, even though all its versions are "drained").
    let set = SegmentSet::open(dir.path().to_path_buf(), 1 << 20)
        .await
        .unwrap();

    for k in 1..=5u64 {
        let v = k * 10;
        set.append_batch(vec![entry(k, v).encode().unwrap()], v)
            .await
            .unwrap();
    }

    // An absurdly large durable_version: every version is "drained", but the
    // active segment must NEVER be deleted — zero deletions expected.
    let deleted = set.truncate_below(u64::MAX).await.unwrap();
    assert_eq!(deleted, 0, "active segment must never be truncated");

    let after = set.replay().await.unwrap();
    let versions: Vec<u64> = after.iter().map(|e| e.commit_version).collect();
    assert_eq!(
        versions,
        vec![10, 20, 30, 40, 50],
        "all active-segment records survive truncate_below(MAX)"
    );
    // The active file is physically present.
    assert!(seg_file_count(dir.path()) >= 1);
}

#[tokio::test]
async fn truncate_keeps_active_with_sealed_present() {
    let dir = TempDir::new().unwrap();
    // Threshold admits one ~entry but rotates on the next, so a sealed
    // segment forms while the trailing record stays in a fresh active.
    let set = SegmentSet::open(dir.path().to_path_buf(), 100)
        .await
        .unwrap();

    // Probe one frame's size to confirm the threshold straddles a single
    // record (so the active tail genuinely holds an un-rotated record).
    let probe_len = entry(0, 0).encode().unwrap().len() as u64 + 8;
    assert!(
        probe_len < 100 && probe_len * 2 >= 100,
        "test threshold assumption broke: frame={probe_len}"
    );

    for k in 1..=3u64 {
        let v = k * 10;
        set.append_batch(vec![entry(k, v).encode().unwrap()], v)
            .await
            .unwrap();
    }
    // After 3 appends at threshold 100: records 1+2 sealed, record 3 active.
    assert!(
        seg_file_count(dir.path()) >= 2,
        "expected a sealed + active split"
    );

    // Truncate everything: sealed (v=10,20) drained & deletable; the active
    // record (v=30) must survive even though 30 <= MAX.
    set.truncate_below(u64::MAX).await.unwrap();
    let after = set.replay().await.unwrap();
    assert_eq!(
        after.iter().map(|e| e.commit_version).collect::<Vec<_>>(),
        vec![30],
        "only the active-segment record survives; sealed are dropped"
    );
}

#[tokio::test]
async fn truncate_pins_v0_segment() {
    let dir = TempDir::new().unwrap();
    let set = SegmentSet::open(dir.path().to_path_buf(), 32)
        .await
        .unwrap();

    // First sealed segment: all commit_version == 0 (a pin, I5).
    set.append_batch(vec![entry(1, 0).encode().unwrap()], 0)
        .await
        .unwrap();
    // Force it to seal by writing more so it rotates.
    set.append_batch(vec![entry(2, 0).encode().unwrap()], 0)
        .await
        .unwrap();
    // A later versioned record so we have a non-trivial set.
    set.append_batch(vec![entry(3, 100).encode().unwrap()], 100)
        .await
        .unwrap();

    let before = set.replay().await.unwrap();
    assert_eq!(before.len(), 3);

    // Huge truncate: the v=0 sealed segment must NOT be deleted (pin).
    set.truncate_below(u64::MAX).await.unwrap();

    let after = set.replay().await.unwrap();
    // The v=0 records survive (their segment is pinned).
    let v0_count = after.iter().filter(|e| e.commit_version == 0).count();
    assert!(
        v0_count >= 1,
        "v=0 (pinned) records must survive truncate_below(MAX), got {:?}",
        after.iter().map(|e| e.commit_version).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn open_recovers_existing_segments() {
    let dir = TempDir::new().unwrap();

    let entries: Vec<WalEntryV2> = (1..=9u64).map(|i| entry(i, i * 10)).collect();
    {
        let set = SegmentSet::open(dir.path().to_path_buf(), 40)
            .await
            .unwrap();
        for e in &entries {
            set.append_batch(vec![e.encode().unwrap()], e.commit_version)
                .await
                .unwrap();
        }
        set.sync().await.unwrap();
        // Drop the set — reopen the same directory.
    }

    let reopened = SegmentSet::open(dir.path().to_path_buf(), 40)
        .await
        .unwrap();
    let replayed = reopened.replay().await.unwrap();
    assert_eq!(replayed, entries, "reopen must replay all records in order");

    // max_committed restored across sealed + active.
    assert_eq!(reopened.max_committed(), 90);

    // And truncation still works after a reopen: drop everything sealed
    // below 55, the survivors (v >= 60) remain.
    reopened.truncate_below(55).await.unwrap();
    let after = reopened.replay().await.unwrap();
    assert!(
        after.iter().all(|e| e.commit_version > 55),
        "post-reopen truncate must drop drained sealed, got {:?}",
        after.iter().map(|e| e.commit_version).collect::<Vec<_>>()
    );
}

/// Group-commit version flow: after `WalGroupCommit::append(payload, v, tier)`
/// the underlying segment's `max_committed()` reflects `v` (the watermark
/// threads end-to-end through the append path).
#[tokio::test]
async fn group_commit_threads_version_to_segment() {
    let dir = TempDir::new().unwrap();
    let seg = WalSegment::open(dir.path().join("00000000.wal"))
        .await
        .unwrap();
    let sink = Arc::new(WalSink::File(seg));
    let gc = WalGroupCommit::new(Arc::clone(&sink));

    gc.append(entry(1, 42).encode().unwrap(), 42, WalDurability::Synced)
        .await
        .unwrap();

    // The leader folded the window's max commit_version into the sink, so the
    // segment's watermark now sees >= 42 — the version threaded end-to-end
    // through append(payload, v, tier) → append_batch(payloads, max_version).
    assert!(
        sink.max_committed() >= 42,
        "segment max_committed must reflect the appended version, got {}",
        sink.max_committed()
    );
}
