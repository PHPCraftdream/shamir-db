use super::helpers::{count_history_entries, make_gate, make_mvcc_with_gate};
use super::test_stores::counting_store::CountingStore;
use crate::mvcc_store::{MvccStore, Retention};
use bytes::Bytes;
use shamir_storage::types::RecordKey;
use shamir_storage::types::Store;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Helper: build an MvccStore backed by a CountingStore so we can
/// assert scan_prefix_stream call counts.
fn make_counting_mvcc(
    gate: Arc<crate::repo_tx_gate::RepoTxGate>,
) -> (MvccStore, Arc<CountingStore>) {
    let store = Arc::new(CountingStore::new());
    let mvcc = MvccStore::new(store.clone() as Arc<dyn Store>, gate);
    (mvcc, store)
}

// ================================================================
// L6 — targeted-remove fast path tests.
// ================================================================

/// CurrentOnly, no snapshots, rewrite-same-key: vacuum does NOT call
/// scan_prefix_stream (the L6 targeted-remove fast path fires).
#[tokio::test]
async fn currentonly_rewrite_no_prefix_scan() {
    let gate = make_gate();
    let (mvcc, store) = make_counting_mvcc(gate);
    // Default retention is CurrentOnly.

    let key = Bytes::from("targeted");
    // First write — append-only, old_v == 0 → vacuum is a no-op, no scan.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    assert_eq!(
        store.scan_prefix_count.load(Ordering::Relaxed),
        0,
        "append-only write must not scan"
    );

    // Second write — old_v > 0, targeted remove fires, no scan.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v2"))
        .await
        .unwrap();
    assert_eq!(
        store.scan_prefix_count.load(Ordering::Relaxed),
        0,
        "CurrentOnly rewrite must not scan (targeted remove)"
    );

    // Third write — same.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v3"))
        .await
        .unwrap();
    assert_eq!(
        store.scan_prefix_count.load(Ordering::Relaxed),
        0,
        "CurrentOnly rewrite must not scan (targeted remove, third write)"
    );

    // Verify correctness: A10 anchor deferral keeps 2 entries (current +
    // deferred previous). The fast path still does NOT scan.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 2,
        "CurrentOnly with A10 anchor deferral: current + deferred previous = 2"
    );
}

/// A version pinned by a live snapshot is NOT reclaimed — the targeted
/// fast path does NOT fire when a snapshot is active.
#[tokio::test]
async fn snapshot_pins_old_version_no_targeted_remove() {
    let gate = make_gate();
    let (mvcc, store) = make_counting_mvcc(gate.clone());

    let key = Bytes::from("snap_pin");
    // Write v1.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);

    // Open a snapshot at v1 — pins min_alive.
    let snap = gate.open_snapshot().await;
    let snap_v = snap.version();
    assert_eq!(snap_v, v1);

    // Overwrite with v2. Targeted fast path must NOT fire (snapshot active).
    // The scan path runs instead.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v2"))
        .await
        .unwrap();
    assert!(
        store.scan_prefix_count.load(Ordering::Relaxed) > 0,
        "with a live snapshot, vacuum must fall back to scan path"
    );

    // The snapshot at v1 still reads v1 (sacred floor).
    let result = mvcc.get_at(&key, snap_v).await.unwrap();
    assert_eq!(
        result,
        Some(Bytes::from("v1")),
        "live snapshot must still read the pinned prior version"
    );

    // Drop snapshot, write v3 — the first write after snapshot drop uses the
    // scan path (to clean up accumulated old versions from the snapshot epoch).
    drop(snap);
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v3"))
        .await
        .unwrap();

    // After the scan-path cleanup, subsequent writes use targeted remove.
    let prev_count = store.scan_prefix_count.load(Ordering::Relaxed);
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v4"))
        .await
        .unwrap();
    assert_eq!(
        store.scan_prefix_count.load(Ordering::Relaxed),
        prev_count,
        "second write after snapshot drop must use targeted remove (no new scan)"
    );
}

/// Anchor preserved under keep_history + live snapshot: the vacuum scan
/// path keeps the anchor (largest version < min_alive).
#[tokio::test]
async fn anchor_preserved_with_keep_history_and_snapshot() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    // Switch to keep_history(max_count = 5) so scan path is always used.
    mvcc.set_retention(Retention {
        max_count: Some(5),
        max_age_secs: None,
        min_count: None,
    })
    .unwrap();

    let key = Bytes::from("anchor_key");
    // Write v1..v3.
    for i in 1..=3u32 {
        mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }
    let v3 = mvcc.version_of(&key);

    // Open snapshot at v3.
    let snap = gate.open_snapshot().await;
    assert_eq!(snap.version(), v3);

    // Write v4..v8 (exceeds max_count=5 with some versions).
    for i in 4..=8u32 {
        mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    // Snapshot at v3 must still read v3 (anchor protects it).
    let result = mvcc.get_at(&key, v3).await.unwrap();
    assert_eq!(
        result,
        Some(Bytes::from("v3")),
        "anchor must protect the version readable by the live snapshot"
    );
}

/// Append-only (cell.current_version == 0) — vacuum is a no-op, no scan.
#[tokio::test]
async fn append_only_no_vacuum_scan() {
    let gate = make_gate();
    let (mvcc, store) = make_counting_mvcc(gate);

    // Write distinct keys — each is an append (old_v == 0).
    for i in 0..5u32 {
        let key = Bytes::from(format!("new_key_{i}"));
        mvcc.set_versioned(RecordKey::from(key), Bytes::from("val"))
            .await
            .unwrap();
    }
    assert_eq!(
        store.scan_prefix_count.load(Ordering::Relaxed),
        0,
        "append-only writes must not trigger any scan"
    );

    // All 5 entries present.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(hist, 5, "5 distinct keys → 5 history entries");
}

/// delete_versioned + CurrentOnly: the tombstone for the new version is
/// present; the old data version is deferred (A10 anchor) — the tombstone
/// is current, the old data version stays as the deferred anchor.
#[tokio::test]
async fn delete_versioned_currentonly_reclaims_old() {
    let gate = make_gate();
    let (mvcc, store) = make_counting_mvcc(gate);

    let key = Bytes::from("del_key");
    // Write v1.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("data"))
        .await
        .unwrap();
    assert_eq!(count_history_entries(&mvcc).await, 1);

    // Delete — fast path fires (no scan). A10: v1 is deferred as anchor,
    // the tombstone becomes current.
    mvcc.delete_versioned(RecordKey::from(key.clone()))
        .await
        .unwrap();
    assert_eq!(
        store.scan_prefix_count.load(Ordering::Relaxed),
        0,
        "delete_versioned CurrentOnly must use fast path (no scan)"
    );

    // A10: 2 entries — tombstone (current) + deferred old data version.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 2,
        "after delete: tombstone (current) + deferred old version = 2"
    );

    // get_current returns None (tombstone).
    let result = mvcc.get_current(RecordKey::from(key)).await.unwrap();
    assert!(result.is_none(), "deleted key must return None");
}
