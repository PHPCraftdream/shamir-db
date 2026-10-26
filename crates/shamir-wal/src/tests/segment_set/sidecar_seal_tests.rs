//! RV-WAL-A025886E — "a segment that has a sidecar never changes."
//!
//! A WMT2 sidecar marks a segment as SEALED forever: `seal_and_rotate` writes
//! it only after the new active segment is open, and `SegmentSet::open` seals
//! (never re-activates) the highest segment that carries a valid one. Legacy
//! WMT1 sidecars are never trusted — such segments replay instead.

use std::path::{Path, PathBuf};

use shamir_types::types::record_id::RecordId;
use tempfile::TempDir;

use crate::segment_set::SegmentSet;
use crate::wal_entry_v2::{WalEntryV2, WalOpV2};

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

fn meta_path(dir: &Path, n: u64) -> PathBuf {
    dir.join(format!("{n:08}.meta"))
}

fn wal_path(dir: &Path, n: u64) -> PathBuf {
    dir.join(format!("{n:08}.wal"))
}

/// Hand-encode a LEGACY WMT1 sidecar (`[WMT1][1][max LE][crc]`) — the format
/// pre-dating RV-WAL-A025886E, which must never be trusted.
fn write_wmt1_blocking(seg_path: &Path, max_version: u64) {
    let mut buf = [0u8; 17];
    buf[0..4].copy_from_slice(b"WMT1");
    buf[4] = 1;
    buf[5..13].copy_from_slice(&max_version.to_le_bytes());
    let crc = crc32fast::hash(&buf[0..13]);
    buf[13..17].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(seg_path.with_extension("meta"), buf).unwrap();
}

async fn replay_versions(set: &SegmentSet) -> Vec<u64> {
    set.replay()
        .await
        .unwrap()
        .iter()
        .map(|e| e.commit_version)
        .collect()
}

/// (b) If opening the NEW active segment fails during `seal_and_rotate`, the
/// sealed segment must be left with NO sidecar — under the sidecar-never-
/// changes rule a sidecar would freeze a segment that is still the live
/// append tail after reopen.
///
/// Setup: the set is opened FIRST (tiny `max_bytes` so one append crosses the
/// threshold), then a DIRECTORY is planted at the seq+1 path (`00000001.wal`),
/// then ONE append crosses `max_bytes`: the post-append rotation's new-active
/// open fails (a directory occupies the path), so the whole append returns Err
/// and no sidecar may exist for the still-active seq 0.
///
/// Single revert that makes this fail on old code: moving the sidecar write
/// in `seal_and_rotate` back to BEFORE the new-active `WalSegment::open`
/// leaves `00000000.meta` on disk when that open fails → the no-sidecar
/// assertion fails.
#[tokio::test]
async fn rv_wal_a025886e_failed_new_active_open_writes_no_sidecar() {
    let dir = TempDir::new().unwrap();
    // Open first: seq 0 is the empty active segment. Tiny cap → one append
    // crosses the threshold and triggers a post-append rotation.
    let set = SegmentSet::open(dir.path().to_path_buf(), 8).await.unwrap();
    // Make opening `00000001.wal` deterministically fail: a DIRECTORY already
    // occupies the path, so `OpenOptions::append` errors (injected failure,
    // no sleeps, no real I/O faults).
    std::fs::create_dir(wal_path(dir.path(), 1)).unwrap();

    // This append crosses `max_bytes` → seal_and_rotate → the new-active open
    // fails → the rotation (and the append) must return Err, and seq 0 must
    // NOT have gained a sidecar (the swap never happened; the write happens
    // only after it).
    let res = set
        .append_batch(vec![entry(1, 60).encode().unwrap()], 60)
        .await;
    assert!(
        res.is_err(),
        "rotation must fail while seq1 cannot be opened"
    );
    assert!(
        !meta_path(dir.path(), 0).exists(),
        "a failed new-active open must leave NO sidecar on the sealed segment"
    );
    assert!(
        wal_path(dir.path(), 0).exists(),
        "the still-active seq 0 .wal must be intact after the failed rotation"
    );
}

/// (c) A legacy WMT1 sidecar is never trusted. A stale WMT1 max=10 on a
/// segment that really holds v60 must cause a full replay, so truncation
/// below 55 keeps the v60 data.
///
/// Single revert that makes this fail on old code: restoring WMT1 acceptance
/// in `segment_meta::decode` (old code read `Some(10)`) makes `open` record
/// seq 0's max as 10 and `truncate_below(55)` deletes the segment holding
/// v60 — the final replay assertion fails.
#[tokio::test]
async fn rv_wal_a025886e_wmt1_sidecar_never_trusted_replay_wins() {
    let dir = TempDir::new().unwrap();
    let set = SegmentSet::open(dir.path().to_path_buf(), 8).await.unwrap();
    set.append_batch(vec![entry(1, 60).encode().unwrap()], 60)
        .await
        .unwrap();
    set.sync().await.unwrap();
    drop(set);

    // Overwrite the WMT2 sidecar of sealed seq 0 (true max 60) with a STALE
    // legacy WMT1 claiming max=10.
    write_wmt1_blocking(&wal_path(dir.path(), 0), 10);

    let reopened = SegmentSet::open(dir.path().to_path_buf(), 1 << 20)
        .await
        .unwrap();
    // The stale WMT1 must not make seq 0 reclaimable below 55.
    let deleted = reopened.truncate_below(55).await.unwrap();
    assert_eq!(
        deleted, 0,
        "a WMT1 sidecar must never be trusted: seq 0 (true max 60) must survive"
    );
    assert_eq!(
        replay_versions(&reopened).await,
        vec![60],
        "replay must keep v60 despite the stale WMT1 claim of max=10"
    );
}

/// Path of the highest-seq `NNNNNNNN.wal` in `dir` (the active append tail).
fn highest_wal_path(dir: &Path) -> PathBuf {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let seq = name.strip_suffix(".wal")?.parse::<u64>().ok()?;
            Some((seq, e.path()))
        })
        .max_by_key(|(seq, _)| *seq)
        .map(|(_, p)| p)
        .expect("at least one .wal segment present")
}

/// (d) Sidecar cleanup is best-effort only: with `remove_blocking` FORCED to
/// fail via the `set_remove_fail_for_test` seam, every correctness property of
/// RV-WAL-A025886E still holds. Phase (i) is the (a) scenario — highest
/// segment with a valid WMT2 sidecar is sealed and seq+1 is created. Phase
/// (ii) plants a legacy WMT1 sidecar on the (new) highest segment: on reopen
/// the active-path cleanup tries to shed it, the removal FAILS (warned, not
/// fatal), the sidecar persists, and appends/replay/truncation stay correct.
///
/// Single revert that makes this fail on old code: removing the highest-
/// segment WMT2 seal check in `SegmentSet::open` re-activates the WMT2-bearing
/// segment, so "appends land in seq 1" and "sidecar bytes unchanged" fail
/// (and with removals forced to fail, the old code could not even shed the
/// sidecar it relied on removing).
#[tokio::test]
async fn rv_wal_a025886e_removal_failure_never_affects_correctness() {
    crate::segment_meta::set_remove_fail_for_test(true);
    let dir = TempDir::new().unwrap();

    // ---- Phase (i): valid WMT2 on the highest segment → sealed, seq+1. ----
    {
        let set = SegmentSet::open(dir.path().to_path_buf(), 1 << 20)
            .await
            .unwrap();
        set.append_batch(vec![entry(1, 10).encode().unwrap()], 10)
            .await
            .unwrap();
        set.sync().await.unwrap();
        drop(set);
    }
    crate::segment_meta::write_blocking(&wal_path(dir.path(), 0), 10).unwrap();
    let meta_before = std::fs::read(meta_path(dir.path(), 0)).unwrap();

    let set = SegmentSet::open(dir.path().to_path_buf(), 32)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(meta_path(dir.path(), 0)).unwrap(),
        meta_before,
        "removal failure must not alter the sidecar of a sealed segment"
    );
    for k in 2..=6u64 {
        let v = k * 10; // 20..60
        set.append_batch(vec![entry(k, v).encode().unwrap()], v)
            .await
            .unwrap();
    }
    set.sync().await.unwrap();
    assert!(
        wal_path(dir.path(), 1).exists(),
        "appends must land in the newly created segment 1"
    );
    assert_eq!(
        std::fs::read(meta_path(dir.path(), 0)).unwrap(),
        meta_before,
        "sealed seq 0's sidecar bytes must stay unchanged throughout"
    );
    drop(set);

    let reopened = SegmentSet::open(dir.path().to_path_buf(), 32)
        .await
        .unwrap();
    assert_eq!(
        replay_versions(&reopened).await,
        vec![10, 20, 30, 40, 50, 60],
        "removal failures must not change replay results"
    );
    drop(reopened);

    // ---- Phase (ii): WMT1 on the (new) highest segment; the cleanup shed ----
    // ---- fails (flag still on) but must change nothing else.            ----
    let highest = highest_wal_path(dir.path());
    write_wmt1_blocking(&highest, 10);
    let wmt1_path = highest.with_extension("meta");
    assert!(wmt1_path.exists(), "precondition: WMT1 sidecar planted");

    // Reopen: the WMT1 sidecar is never trusted (replay fallback) and the
    // active-path cleanup tries to remove it — the removal FAILS (injected),
    // is warned, and the sidecar persists.
    let set = SegmentSet::open(dir.path().to_path_buf(), 1 << 20)
        .await
        .unwrap();
    assert!(
        wmt1_path.exists(),
        "with removals forced to fail, the legacy sidecar must persist"
    );
    // Appends still work on the re-opened active segment.
    set.append_batch(vec![entry(7, 70).encode().unwrap()], 70)
        .await
        .unwrap();
    set.sync().await.unwrap();
    drop(set);

    // Final reopen: every version intact, v60 survives truncate_below(55).
    let reopened = SegmentSet::open(dir.path().to_path_buf(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(
        replay_versions(&reopened).await,
        vec![10, 20, 30, 40, 50, 60, 70],
        "a persisting untrusted sidecar must never alter replay"
    );
    reopened.truncate_below(55).await.unwrap();
    let versions = replay_versions(&reopened).await;
    assert!(
        versions.contains(&60),
        "v60 must survive truncate_below(55) despite sidecar-removal failures, got {versions:?}"
    );
    drop(reopened);
    crate::segment_meta::set_remove_fail_for_test(false);
}

#[tokio::test]
async fn rv_wal_22b8d9ca_absent_sidecar_sealed_crc_corruption_is_loud() {
    let dir = TempDir::new().unwrap();
    let set = SegmentSet::open(dir.path().to_path_buf(), 32)
        .await
        .unwrap();
    set.append_batch(vec![entry(1, 11).encode().unwrap()], 11)
        .await
        .unwrap();
    set.append_batch(vec![entry(2, 22).encode().unwrap()], 22)
        .await
        .unwrap();
    drop(set);
    std::fs::remove_file(meta_path(dir.path(), 0)).unwrap();
    let path = wal_path(dir.path(), 0);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[5] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(SegmentSet::open(dir.path().to_path_buf(), 32)
        .await
        .is_err());
}

#[tokio::test]
async fn rv_wal_707efc66_replay_does_not_create_missing_sealed_file() {
    let dir = TempDir::new().unwrap();
    let set = SegmentSet::open(dir.path().to_path_buf(), 32)
        .await
        .unwrap();
    set.append_batch(vec![entry(1, 11).encode().unwrap()], 11)
        .await
        .unwrap();
    set.append_batch(vec![entry(2, 22).encode().unwrap()], 22)
        .await
        .unwrap();
    let missing = wal_path(dir.path(), 0);
    std::fs::remove_file(&missing).unwrap();
    assert_eq!(
        set.replay()
            .await
            .unwrap()
            .iter()
            .map(|e| e.txn_id)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert!(!missing.exists());
}
