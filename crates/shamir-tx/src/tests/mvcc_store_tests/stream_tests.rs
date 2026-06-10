use super::helpers::{make_gate, make_mvcc};
use bytes::Bytes;
use futures::StreamExt;

// ========================================================================
// "MVCC owns current state" read-seam (get_current / current_stream).
// These tests pin the behaviour contract: both methods read the single
// version log (`history`). There is no `main` store.
// ========================================================================

/// `current_stream(batch)` yields all current `(key, value)` pairs from
/// the log. The log is the single source of truth; verifies the 3 written
/// pairs appear.
#[tokio::test]
async fn current_stream_lists_all_current() {
    use std::collections::BTreeMap;

    let mvcc = make_mvcc();

    let pairs: &[(Bytes, Bytes)] = &[
        (Bytes::from("s-a"), Bytes::from("va")),
        (Bytes::from("s-b"), Bytes::from("vb")),
        (Bytes::from("s-c"), Bytes::from("vc")),
    ];
    for (k, v) in pairs {
        mvcc.set_versioned(k.clone(), v.clone()).await.unwrap();
    }

    // Collect the seam stream into a map.
    let mut via_seam: BTreeMap<Vec<u8>, Bytes> = BTreeMap::new();
    let seam_stream = mvcc.current_stream(64);
    futures::pin_mut!(seam_stream);
    while let Some(batch) = seam_stream.next().await {
        for (k, v) in batch.unwrap() {
            via_seam.insert(k.to_vec(), v);
        }
    }

    // Non-vacuous: exactly the 3 written current values are present in the log.
    assert_eq!(
        via_seam.len(),
        3,
        "exactly the 3 written keys must be current in the log"
    );
    for (k, v) in pairs {
        assert_eq!(
            via_seam.get(k.as_ref()),
            Some(v),
            "missing current value for {k:?}"
        );
    }
}

/// C2: `current_stream` reads the log. It picks the MAX version per key
/// (the current), suppressing older versions.
#[tokio::test]
async fn c2_current_stream_reads_log_not_main() {
    use crate::mvcc_store::Retention;
    use std::collections::BTreeMap;

    let mvcc = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();

    // k1 written twice (consecutively) → log holds v1(old) AND v2(current).
    mvcc.set_versioned(Bytes::from("k1"), Bytes::from("a"))
        .await
        .unwrap();
    mvcc.set_versioned(Bytes::from("k1"), Bytes::from("b"))
        .await
        .unwrap();
    mvcc.set_versioned(Bytes::from("k2"), Bytes::from("c"))
        .await
        .unwrap();
    mvcc.set_versioned(Bytes::from("k3"), Bytes::from("d"))
        .await
        .unwrap();

    let mut got: BTreeMap<Vec<u8>, Bytes> = BTreeMap::new();
    let stream = mvcc.current_stream(64);
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (k, v) in batch.unwrap() {
            got.insert(k.to_vec(), v);
        }
    }

    assert_eq!(got.len(), 3, "C2: exactly 3 current keys");
    // k1's CURRENT is "b" (the max version), NOT the old "a".
    assert_eq!(
        got.get(&b"k1"[..]),
        Some(&Bytes::from("b")),
        "C2: max version"
    );
    assert_eq!(got.get(&b"k2"[..]), Some(&Bytes::from("c")));
    assert_eq!(got.get(&b"k3"[..]), Some(&Bytes::from("d")));
}

/// C2 regression: `current_stream` must NOT panic when the current-key set
/// exceeds one batch. The first streaming-group-by implementation paniced
/// (`unreachable!()`) on the second pull whenever `out_batch` filled to
/// `batch_size` and returned a `Streaming` continuation state.
#[tokio::test]
async fn c2_current_stream_exceeds_batch_no_panic() {
    use std::collections::BTreeSet;

    let mvcc = make_mvcc();
    // 5 distinct current keys, streamed with batch_size = 2 (3 output
    // batches: 2 + 2 + 1). The buggy code paniced on the 2nd pull.
    for i in 0..5u32 {
        mvcc.set_versioned(Bytes::from(format!("bk{i}")), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    let mut keys: BTreeSet<Vec<u8>> = BTreeSet::new();
    let stream = mvcc.current_stream(2);
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (k, _v) in batch.unwrap() {
            keys.insert(k.to_vec());
        }
    }
    assert_eq!(
        keys.len(),
        5,
        "C2: current_stream must yield all 5 keys across batches without panicking"
    );
}

/// MVCC-2 characterization via PausableStore — now asserts the fix.
///
/// `PausableStore` suspends `history.set()` (the log append for NEW) —
/// which is AFTER `publish_cell` (cell advanced to `v_new`). A snapshot
/// opened inside this pause sees `cur_v = v_new > snap_v` → log
/// range-scan → finds OLD at v_seed. The phantom read cannot occur.
///
/// Sequence:
///   [set_versioned(NEW)] publish_cell(v_new) → history.set(NEW)
///                                                      ↑ pause here
///   [snapshot opens at v_after_seed]
///   [release → history.set commits NEW to the log]
///   [get_at(key, v_after_seed)] → cur_v=v_new > v_after_seed → history → OLD
///
/// MVCC-2 guarantee: publish_cell advances BEFORE the log write, so
/// any snapshot opened during the log write sees the bumped cell and
/// range-scans the log → finds OLD. Safe by construction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mvcc2_real_interleaving_toctou_characterization() {
    use super::test_stores::pausable_store::PausableStore;
    use crate::mvcc_store::MvccStore;
    use shamir_storage::types::Store;
    use std::sync::Arc;

    let key = Bytes::from("toctou_key");
    let old_val = Bytes::from("OLD");
    let new_val = Bytes::from("NEW");

    // --- Setup ---
    // PausableStore is wired as `history` (the single log / sole write target).
    let pausable = Arc::new(PausableStore::new());
    let gate = make_gate();
    let mvcc = Arc::new(MvccStore::new(
        pausable.clone() as Arc<dyn Store>,
        gate.clone(),
    ));

    // Seed: write OLD with no snapshots open. The cell IS published with
    // the seed version and OLD lands in the log.
    mvcc.set_versioned(key.clone(), old_val.clone())
        .await
        .unwrap();
    let v_seed = mvcc.version_of(&key);
    assert!(v_seed > 0);
    // Publish so a snapshot can capture the current committed version.
    let v_after_seed = gate.assign_next_version();
    gate.publish_committed(v_after_seed);

    // The seed write published its version into the cell.
    assert_eq!(
        mvcc.version_of(&key),
        v_seed,
        "FINAL-A: seed write publishes its version into the cell"
    );
    // Confirm no snapshots are open before arming.
    assert!(
        gate.active_snapshots_empty(),
        "precondition: no snapshots open before arming"
    );

    // --- Arm PausableStore ---
    // The next `set()` call on `history` will pause before writing.
    // The sequence for set_versioned(NEW) is:
    //   publish_cell(v_new) → history.set(NEW) ← PAUSE
    // When `entered` fires, the cell already carries v_new but NEW is
    // not yet in the log.
    pausable.arm();

    // Clone refs for the write task.
    let mvcc_w = Arc::clone(&mvcc);
    let key_w = key.clone();
    let new_val_w = new_val.clone();

    // --- Spawn write task ---
    let write_handle = tokio::spawn(async move {
        mvcc_w.set_versioned(key_w, new_val_w).await.unwrap();
    });

    // --- Wait for write to be inside the pause ---
    // `entered` fires inside `history.set`, which is AFTER publish_cell.
    // The cell holds v_new; OLD is still the latest committed entry in the log.
    pausable.entered.notified().await;

    // --- Open snapshot inside the window ---
    // snap_v = last_committed = v_after_seed (published above).
    // From this snapshot's perspective, NEW has NOT landed in the log yet.
    let snap = gate.open_snapshot().await;
    let snap_v = snap.version();
    assert_eq!(
        snap_v, v_after_seed,
        "snapshot must open at v_after_seed (the gap version)"
    );

    // publish_cell has already advanced the cell to v_new.
    let v_new = mvcc.version_of(&key);
    assert!(
        v_new > snap_v,
        "FINAL-A: cell already carries v_new > snap_v (publish_cell ran before the pause)"
    );

    // --- Release: let the log write commit NEW ---
    pausable.release();
    write_handle.await.unwrap();

    // NEW is now in the log. The cell still carries v_new.
    assert_eq!(
        mvcc.version_of(&key),
        v_new,
        "FINAL-A: cell carries v_new after the log write commits"
    );

    // --- The (now-correct) characterization moment ---
    // get_at(key, snap_v):
    //   cur_v = v_new > snap_v → SLOW PATH → scan history → finds OLD at v_seed.
    //
    // OLD is in the log (written at v_seed). The snapshot opened at
    // v_after_seed (< v_new) correctly sees OLD. The phantom read of NEW
    // cannot occur: MVCC-2 is closed by construction.
    let seen = mvcc.get_at(&key, snap_v).await.unwrap();
    assert_eq!(
        seen,
        Some(old_val),
        "FINAL-A: snapshot opened inside the log-write window sees OLD, \
         not the post-snapshot NEW — MVCC-2 cannot occur"
    );
}
