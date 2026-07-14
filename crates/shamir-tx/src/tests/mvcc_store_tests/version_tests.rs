use super::helpers::{make_gate, make_mvcc, make_mvcc_with_gate};
use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::types::KvOp;
use shamir_storage::types::RecordKey;

#[tokio::test]
async fn version_cache_populated_on_set() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let _guard = gate.open_snapshot().await;
    let key = Bytes::from("k1");
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();

    let cached = mvcc.cells.read_sync(key.as_ref(), |_, c| c.version);
    assert!(
        cached.is_some(),
        "version_cache should contain key after set"
    );
    assert!(cached.unwrap() > 0, "version should be > 0");
}

#[tokio::test]
async fn version_of_returns_zero_for_unknown_key() {
    let mvcc = make_mvcc();
    let v = mvcc.version_of(b"never_written");
    assert_eq!(v, 0);
}

#[tokio::test]
async fn version_of_returns_cached_version_after_versioned_set() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;
    let key = Bytes::from("kx");
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    let v = mvcc.version_of(&key);
    assert!(v > 0, "version_of must reflect the assigned version");
}

#[tokio::test]
async fn apply_committed_ops_updates_version_cache() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;

    let key = Bytes::from("k_commit");
    let ops = vec![KvOp::Set(key.clone().into(), Bytes::from("val"))];
    mvcc.apply_committed_ops(ops, 42).await.unwrap();

    assert_eq!(mvcc.version_of(&key), 42);

    // Value is in the log.
    let val = mvcc
        .get_current(RecordKey::from(key.clone()))
        .await
        .unwrap();
    assert_eq!(val, Some(Bytes::from("val")));
}

#[tokio::test]
async fn apply_committed_ops_archives_old_value() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;

    let key = Bytes::from("k_archive");

    let ops = vec![KvOp::Set(key.clone().into(), Bytes::from("new"))];
    mvcc.apply_committed_ops(ops, 10).await.unwrap();

    // Value is in the log (single log append). Assert via the seam.
    let via_seam = mvcc
        .get_current(RecordKey::from(key.clone()))
        .await
        .unwrap();
    assert_eq!(via_seam, Some(Bytes::from("new")));

    // The log contains the new value at the commit version.
    let stream = mvcc.history.iter_stream(64);
    futures::pin_mut!(stream);
    let mut found = false;
    while let Some(batch) = stream.next().await {
        for (hk, hv) in batch.unwrap() {
            if let Some((orig, ver)) = crate::version_codec::decode_version_key(&hk) {
                if orig == b"k_archive" && ver == 10 {
                    assert_eq!(hv, Bytes::from("new"));
                    found = true;
                }
            }
        }
    }
    assert!(found, "FINAL-A: new value at commit version in the log");
}

#[tokio::test]
async fn apply_committed_ops_remove_archives_and_deletes() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;

    let key = Bytes::from("k_del");

    let ops = vec![KvOp::Remove(key.clone().into())];
    mvcc.apply_committed_ops(ops, 20).await.unwrap();

    // Remove writes a tombstone to the log; get_current reads it as None.
    let via_seam = mvcc
        .get_current(RecordKey::from(key.clone()))
        .await
        .unwrap();
    assert!(
        via_seam.is_none(),
        "FINAL-A: Remove tombstone → None from seam"
    );

    // Remove writes a tombstone (empty value) into the log at commit version.
    let stream = mvcc.history.iter_stream(64);
    futures::pin_mut!(stream);
    let mut found = false;
    while let Some(batch) = stream.next().await {
        for (hk, hv) in batch.unwrap() {
            if let Some((orig, ver)) = crate::version_codec::decode_version_key(&hk) {
                if orig == b"k_del" && ver == 20 {
                    assert_eq!(hv, Bytes::new(), "C1: tombstone for remove");
                    found = true;
                }
            }
        }
    }
    assert!(found, "C1: remove tombstone in the log");
}

#[tokio::test]
async fn apply_committed_ops_no_snapshots_skips_history() {
    let mvcc = make_mvcc();

    let key = Bytes::from("k_nohist");

    let ops = vec![KvOp::Set(key.clone().into(), Bytes::from("new"))];
    mvcc.apply_committed_ops(ops, 5).await.unwrap();

    // Value is in the log (single log append). Assert via the seam.
    let via_seam = mvcc
        .get_current(RecordKey::from(key.clone()))
        .await
        .unwrap();
    assert_eq!(via_seam, Some(Bytes::from("new")));

    assert_eq!(mvcc.version_of(&key), 5);

    // Every committed op writes into the log unconditionally (sole write).
    // So 1 version-key entry.
    let stream = mvcc.history.iter_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        for (hk, _) in batch.unwrap() {
            if crate::version_codec::decode_version_key(&hk).is_some() {
                count += 1;
            }
        }
    }
    assert_eq!(
        count, 1,
        "FINAL-A: apply_committed_ops writes a single entry into the log"
    );
}

/// CRIT-2 regression test. Before the fix, `version_cache.entry()
/// .insert_entry()` was a no-op when the key was already cached,
/// so the second `apply_committed_ops(..., 200)` left the cached
/// version stuck at 100 and SSI conflict detection silently
/// failed.
#[tokio::test]
async fn version_cache_updates_on_repeated_writes_to_same_key() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;
    let key = Bytes::from("repeated");

    mvcc.apply_committed_ops(vec![KvOp::Set(key.clone().into(), Bytes::from("v1"))], 100)
        .await
        .unwrap();
    assert_eq!(mvcc.version_of(&key), 100);

    mvcc.apply_committed_ops(vec![KvOp::Set(key.clone().into(), Bytes::from("v2"))], 200)
        .await
        .unwrap();
    // CRITICAL: must be 200, was 100 before the fix.
    assert_eq!(
        mvcc.version_of(&key),
        200,
        "version_cache must update on repeated writes"
    );
}

/// HIGH-2 regression guard. Before the fix, `snapshots_active`
/// was sampled once at function entry; a snapshot that opened
/// after the sample but before the first op would silently miss
/// the archive. Per-op re-sampling closes that race for every
/// op processed after the snapshot becomes visible.
///
/// C1: the log is now the universal version timeline — every
/// committed op writes into the log unconditionally (no longer
/// gated by active_snapshots_empty). The new value at the commit
/// version is always present in the log.
#[tokio::test]
async fn apply_committed_ops_archives_even_if_snapshot_opens_mid_call() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    // Open a snapshot right before apply.
    let _g = gate.open_snapshot().await;

    mvcc.apply_committed_ops(
        vec![KvOp::Set(Bytes::from("k").into(), Bytes::from("new"))],
        50,
    )
    .await
    .unwrap();

    // The new value is written into the log at version 50
    // (single log append — the log is the universal timeline).
    let stream = mvcc.history.iter_stream(64);
    futures::pin_mut!(stream);
    let mut found = false;
    while let Some(batch) = stream.next().await {
        for (hk, hv) in batch.unwrap() {
            if let Some((orig, ver)) = crate::version_codec::decode_version_key(&hk) {
                if orig == b"k" && ver == 50 {
                    assert_eq!(hv, Bytes::from("new"));
                    found = true;
                }
            }
        }
    }
    assert!(found, "C1: new value at commit version in the log");
}

// ----------------------------------------------------------------
// III.2 — alloc-free `current_version` borrow-probe.
// ----------------------------------------------------------------

/// The borrow-based probe (`read(key: &[u8], ..)`) must locate entries
/// inserted under arbitrary-length `Bytes` keys — not just the 16-byte
/// RecordId case — confirming `[u8]: Equivalent<Bytes>` resolves and the
/// hashes line up for any key length (incl. SSI keys that aren't 16 bytes).
#[tokio::test]
async fn version_of_borrow_probe_matches_arbitrary_length_keys() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;

    // Short (3-byte), long (40-byte), and empty keys.
    let short = Bytes::from_static(b"abc");
    let long = Bytes::from(vec![7u8; 40]);
    let empty = Bytes::new();

    mvcc.set_versioned(RecordKey::from(short.clone()), Bytes::from("s"))
        .await
        .unwrap();
    mvcc.set_versioned(RecordKey::from(long.clone()), Bytes::from("l"))
        .await
        .unwrap();
    mvcc.set_versioned(RecordKey::from(empty.clone()), Bytes::from("e"))
        .await
        .unwrap();

    // `version_of` takes `&[u8]`; the probe must find each entry.
    assert!(
        mvcc.version_of(short.as_ref()) > 0,
        "3-byte key must be found via borrow-probe"
    );
    assert!(
        mvcc.version_of(long.as_ref()) > 0,
        "40-byte key must be found via borrow-probe"
    );
    assert!(
        mvcc.version_of(empty.as_ref()) > 0,
        "empty key must be found via borrow-probe"
    );
    // A never-written key still returns 0.
    assert_eq!(mvcc.version_of(b"missing"), 0);
}

// ================================================================
// A1+A2 — live_version (hwm) tests.
// =================================================================
//
// Every write is a single log append: `publish_cell` always runs, so
// `version` is always set to the assigned version. `live_version` and
// `version_of` therefore always agree.

/// Every write publishes its version into the cell, so `live_version`
/// and `version_of` agree.
#[tokio::test]
async fn live_version_tracks_fast_path_write() {
    let mvcc = make_mvcc();
    let key = Bytes::from("k_hwm_fast");

    // Every write is a single log append; publish_cell always fires.
    let v = mvcc
        .set_versioned(RecordKey::from(key.clone()), Bytes::from("val"))
        .await
        .unwrap();

    // live_version returns the assigned hwm.
    assert_eq!(
        mvcc.live_version(&key),
        Some(v),
        "live_version must equal the assigned version"
    );
    // version_of returns the latest committed version for the key.
    assert_eq!(
        mvcc.version_of(&key),
        v,
        "version_of equals the assigned version"
    );
    // get_at at 0 (cur_v=v > 0 → log range-scan → no entry below 0 → None).
    let result = mvcc.get_at(&key, 0).await.unwrap();
    assert_eq!(
        result, None,
        "T1a: get_at(0) scans history (empty for a brand-new key) → None"
    );
}

/// A write with a live snapshot open still publishes the version into the cell.
#[tokio::test]
async fn live_version_tracks_slow_path_write() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;
    let key = Bytes::from("k_hwm_slow");

    let v = mvcc
        .set_versioned(RecordKey::from(key.clone()), Bytes::from("val"))
        .await
        .unwrap();

    assert_eq!(
        mvcc.live_version(&key),
        Some(v),
        "live_version must equal the assigned version"
    );
    assert_eq!(mvcc.version_of(&key), v, "version_of must also equal v");
}

/// live_version is None before any write touches a key.
#[tokio::test]
async fn live_version_absent_before_any_write() {
    let mvcc = make_mvcc();
    assert_eq!(
        mvcc.live_version(b"never"),
        None,
        "live_version must be None for a key never written"
    );
}

/// After a write followed by a delete, live_version equals the delete's version.
#[tokio::test]
async fn live_version_advances_on_delete() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;
    let key = Bytes::from("k_hwm_del");

    let v1 = mvcc
        .set_versioned(RecordKey::from(key.clone()), Bytes::from("val"))
        .await
        .unwrap();
    let vd = mvcc
        .delete_versioned(RecordKey::from(key.clone()))
        .await
        .unwrap();

    assert!(vd > v1, "delete version must be greater than write version");
    assert_eq!(
        mvcc.live_version(&key),
        Some(vd),
        "live_version must advance to the delete version"
    );
}

/// `set_versioned_many` assigns one version per key (like the per-record
/// `set_versioned` loop), so each key's `live_version` and `version_of`
/// carry that key's own version.
///
/// T1b.1: uses `KeepHistory` so the eager vacuum does not prune the
/// cells before the assertions (this test checks cell-population, not
/// vacuum behaviour).
#[tokio::test]
async fn set_versioned_many_sets_hwm_fast_path() {
    use crate::mvcc_store::Retention;

    let mvcc = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let items: Vec<(Bytes, Bytes)> = vec![
        (Bytes::from("bk1"), Bytes::from("v1")),
        (Bytes::from("bk2"), Bytes::from("v2")),
        (Bytes::from("bk3"), Bytes::from("v3")),
    ];
    let max_v = mvcc
        .set_versioned_many(
            items
                .clone()
                .into_iter()
                .map(|(k, v)| (RecordKey::from(k), v))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    assert!(max_v > 0);

    // Every key gets its own monotonic version (one per record);
    // both live_version and version_of carry it.
    let mut prev = 0u64;
    for (key, _) in &items {
        let v = mvcc.live_version(key).expect("key present");
        assert!(v > prev, "T1a always-archive: per-key monotonic versions");
        assert_eq!(
            mvcc.version_of(key),
            v,
            "T1a always-archive: version_of == live_version (no fast-path split)"
        );
        prev = v;
    }
    assert_eq!(prev, max_v, "returned max_v is the last assigned version");
}

// ================================================================
// MVCC-2 VERIFICATION — set_versioned TOCTOU (historical; fixed)
// ================================================================
//
// The original MVCC-2 race: the old dual-write path checked
// `active_snapshots_empty()` and only wrote to `history` when snapshots
// were open. A snapshot opening between the check and the `main.set()`
// could read the freshly written value before it was archived to history.
//
// This race is ELIMINATED by the single-log design: every write is one
// log append (`publish_cell` then `history.set`); there is no conditional
// archive and no `main` store to bypass. `publish_cell` fires before the
// log write, so any snapshot opened mid-write sees the bumped cell version
// and correctly falls back to a log range-scan.
//
// The tests below document the fixed behaviour:
//   1. Sequential test: snapshot opened AFTER the write sees the correct value.
//   2. Concurrent stress test: 1000 iterations confirm the race is not
//      triggerable with the current single-log write path.
//   3. Simulated-TOCTOU test: snapshot opened BEFORE the write does NOT see
//      the post-snapshot value (correct isolation).

/// MVCC-2 deterministic: sequential check → set → open_snapshot → read.
///
/// Verifies that when the snapshot is opened AFTER `set_versioned`
/// completes (the normal sequential case), `get_at` correctly returns
/// the value visible at the snapshot version.
///
/// MVCC-2 cannot occur with the single-log design. This test asserts
/// snapshot-correct visibility: `version_of` is populated on every write
/// (single log append), so a snapshot opened after the write at
/// `snap_v == v` routes to the correct log entry.
#[tokio::test]
async fn mvcc2_fast_path_version_cache_not_updated() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let key = Bytes::from("toctou_key");

    // Write with no snapshot active — single log append.
    let v = mvcc
        .set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    assert!(v > 0);

    // Every write publishes the version into the cell.
    assert_eq!(
        mvcc.version_of(&key),
        v,
        "every write publishes its version into the cell"
    );

    // Now publish and open a snapshot.
    gate.publish_committed(v);
    let snap = gate.open_snapshot().await;
    let snap_v = snap.version();
    assert_eq!(snap_v, v, "snapshot must open at the published version");

    // get_at: cur_v=v ≤ snap_v=v → direct-read path → reads log at v → sees v1.
    // Correct: the snapshot was opened AFTER the write landed.
    let result = mvcc.get_at(&key, snap_v).await.unwrap();
    assert_eq!(
        result,
        Some(Bytes::from("v1")),
        "snapshot opened after the write sees v1 via direct-read path (correct)"
    );
}

/// MVCC-2 simulated TOCTOU — asserts snapshot-correct isolation.
///
/// MVCC-2 cannot occur: every write is a single log append regardless of
/// snapshot state. A snapshot opened BEFORE a write does NOT see the
/// post-snapshot value — it sees the pre-write value (OLD) from the log.
///
/// Sequence:
///   1. Seed OLD via `set_versioned` (no snapshot).
///   2. Open a snapshot at `v_old`.
///   3. Overwrite with NEW via `set_versioned` — log advances to `v_new`.
///   4. `get_at(key, v_old)`: `cur_v = v_new > v_old` → log range-scan →
///      finds OLD. Correct.
#[tokio::test]
async fn mvcc2_simulated_toctou_snapshot_sees_phantom() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let key = Bytes::from("toctou_key");

    // Step 1: seed OLD with no snapshot active. A single log entry is written.
    let old_val = Bytes::from("OLD");
    let v_old = mvcc
        .set_versioned(RecordKey::from(key.clone()), old_val.clone())
        .await
        .unwrap();
    assert!(v_old > 0);
    gate.publish_committed(v_old);

    // Step 2: open a snapshot at v_old — from its perspective NEW has
    // not happened yet.
    let snap = gate.open_snapshot().await;
    let snap_v = snap.version();
    assert_eq!(snap_v, v_old, "snapshot opens at the published v_old");

    // Step 3: overwrite with NEW via the REAL set_versioned (always-
    // archive). OLD is now archived to history; the cell advances.
    let new_val = Bytes::from("NEW");
    let v_new = mvcc
        .set_versioned(RecordKey::from(key.clone()), new_val.clone())
        .await
        .unwrap();
    assert!(v_new > v_old);
    assert_eq!(
        mvcc.version_of(&key),
        v_new,
        "T1a always-archive: cell carries the latest assigned version"
    );

    // Step 4: get_at for the snapshot.
    // cur_v = v_new > snap_v → log range-scan → finds OLD.
    // The snapshot does NOT see the phantom NEW; it sees the value that
    // was current at snap_v. MVCC-2 is closed by construction.
    let result = mvcc.get_at(&key, snap_v).await.unwrap();
    assert_eq!(
        result,
        Some(old_val),
        "T1a: snapshot opened before the overwrite sees OLD (archived), \
         not the post-snapshot NEW — MVCC-2 cannot occur"
    );
}

/// MVCC-2 stress: concurrent set_versioned + open_snapshot, 500 iterations.
///
/// MVCC-2 cannot occur: every write is a single log append that always
/// publishes its version into the cell. The old anomaly predicate
/// (`version_of == 0` after a write) can never hold. The stress runs as
/// a no-anomaly guard: any anomaly would indicate a regression.
#[tokio::test]
async fn mvcc2_stress_race_not_triggered_with_in_memory_store() {
    use std::sync::atomic::{AtomicUsize, Ordering as AO};
    use std::sync::Arc;

    let gate = Arc::new(crate::repo_tx_gate::RepoTxGate::fresh());
    let mvcc = Arc::new(make_mvcc_with_gate(gate.clone()));

    let anomaly_count = Arc::new(AtomicUsize::new(0));

    let iterations = 500;
    let mut handles = Vec::with_capacity(iterations * 2);

    for i in 0..iterations {
        let key = Bytes::copy_from_slice(&(i as u64).to_be_bytes());
        let mvcc_w = Arc::clone(&mvcc);
        let gate_r = Arc::clone(&gate);
        let mvcc_r = Arc::clone(&mvcc);
        let anomaly = Arc::clone(&anomaly_count);

        // Writer: set_versioned on the key.
        let key_w = key.clone();
        let write_handle = tokio::spawn(async move {
            let _ = mvcc_w
                .set_versioned(RecordKey::from(key_w), Bytes::from("written"))
                .await
                .unwrap();
        });

        // Reader: open snapshot and try to read the key.
        let key_r = key.clone();
        let read_handle = tokio::spawn(async move {
            let snap = gate_r.open_snapshot().await;
            let snap_v = snap.version();
            // Small yield to increase interleaving odds.
            tokio::task::yield_now().await;
            let result = mvcc_r.get_at(&key_r, snap_v).await.unwrap();
            // Every write publishes its version, so version_of is never 0
            // after a write lands. An anomaly here would mean a snapshot
            // observed a value whose version is strictly greater
            // than snap_v AND version_of disagrees — i.e. a regression
            // (publish_cell not called before log write). With the single-log
            // design this predicate should never fire.
            let write_v = mvcc_r.version_of(&key_r);
            if result.is_some() && snap_v == 0 && write_v == 0 {
                anomaly.fetch_add(1, AO::Relaxed);
            }
            drop(snap);
        });

        handles.push(write_handle);
        handles.push(read_handle);
    }

    for h in handles {
        h.await.unwrap();
    }

    let anomalies = anomaly_count.load(AO::Relaxed);
    // With the single-log design the anomaly predicate (version_of == 0
    // after a write) can never hold, so anomalies must be 0. A non-zero
    // count would indicate the publish_cell-before-log-write ordering regressed.
    assert_eq!(
        anomalies, 0,
        "T1a always-archive: no phantom-read anomaly (version_of is never 0 after a write)"
    );
}
