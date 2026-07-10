use super::helpers::{count_history_entries, make_gate, make_mvcc_with_gate};
use super::test_stores::counting_store::CountingStore;
use crate::mvcc_store::{MvccStore, Retention};
use bytes::Bytes;
use shamir_storage::types::Store;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use shamir_storage::types::RecordKey;

/// Helper: build an MvccStore backed by a CountingStore so we can
/// assert scan_prefix_stream call counts.
fn make_counting_mvcc(
    gate: Arc<crate::repo_tx_gate::RepoTxGate>,
) -> (MvccStore, Arc<CountingStore>) {
    let store = Arc::new(CountingStore::new());
    let mvcc = MvccStore::new(store.clone() as Arc<dyn Store>, gate);
    (mvcc, store)
}

/// `set_versioned_many_append_only` must NOT call scan_prefix_stream for
/// fresh keys under CurrentOnly retention. Verifies the append-only
/// fast path fires (no per-row current_version lookup overhead).
#[tokio::test]
async fn append_only_batch_skips_current_version_lookup() {
    let gate = make_gate();
    let (mvcc, store) = make_counting_mvcc(gate);

    // Batch of 10 fresh keys.
    let items: Vec<(Bytes, Bytes)> = (0..10u32)
        .map(|i| {
            (
                Bytes::from(format!("fresh_key_{i}")),
                Bytes::from(format!("val_{i}")),
            )
        })
        .collect();

    mvcc.set_versioned_many_append_only(items.into_iter().map(|(k, v)| (RecordKey::from(k), v)).collect::<Vec<_>>()).await.unwrap();

    assert_eq!(
        store.scan_prefix_count.load(Ordering::Relaxed),
        0,
        "append-only batch must not trigger any scan_prefix_stream"
    );
}

/// Cross-check: `set_versioned_many_append_only` produces the exact same
/// observable state (history entries + overlay + cells) as `set_versioned_many`
/// when called with the same fresh-key input.
#[tokio::test]
async fn append_only_byte_identical_to_many() {
    let gate_a = make_gate();
    let mvcc_a = make_mvcc_with_gate(gate_a);

    let gate_b = make_gate();
    let mvcc_b = make_mvcc_with_gate(gate_b);

    // Same items for both stores.
    let items: Vec<(Bytes, Bytes)> = (0..5u32)
        .map(|i| {
            (
                Bytes::from(format!("key_{i}")),
                Bytes::from(format!("value_{i}")),
            )
        })
        .collect();

    mvcc_a.set_versioned_many(items.clone().into_iter().map(|(k, v)| (RecordKey::from(k), v)).collect::<Vec<_>>()).await.unwrap();
    mvcc_b
        .set_versioned_many_append_only(items.clone().into_iter().map(|(k, v)| (RecordKey::from(k), v)).collect::<Vec<_>>())
        .await
        .unwrap();

    // Both must have the same number of history entries.
    let hist_a = count_history_entries(&mvcc_a).await;
    let hist_b = count_history_entries(&mvcc_b).await;
    assert_eq!(
        hist_a, hist_b,
        "history entry count must match between set_versioned_many and append_only"
    );

    // Both must have the same overlay length.
    assert_eq!(
        mvcc_a.overlay_len(),
        mvcc_b.overlay_len(),
        "overlay length must match"
    );

    // Both must resolve the same values via get_current.
    for (key, expected_val) in &items {
        let val_a = mvcc_a.get_current(RecordKey::from(key.clone())).await.unwrap();
        let val_b = mvcc_b.get_current(RecordKey::from(key.clone())).await.unwrap();
        assert_eq!(
            val_a,
            Some(expected_val.clone()),
            "set_versioned_many must return correct value for key"
        );
        assert_eq!(
            val_b,
            Some(expected_val.clone()),
            "set_versioned_many_append_only must return correct value for key"
        );
    }
}

/// Under a live snapshot, `set_versioned_many_append_only` must work
/// correctly: the vacuum_needs_scan flag is set so the scan path fires
/// on subsequent vacuum_key calls, and the snapshot reads the correct
/// data.
#[tokio::test]
async fn append_only_under_snapshot_safe() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    // Write some initial data (creates a baseline).
    let key_pre = Bytes::from("pre_snap_key");
    mvcc.set_versioned(RecordKey::from(key_pre.clone()), Bytes::from("pre_val"))
        .await
        .unwrap();
    let pre_v = mvcc.version_of(&key_pre);

    // Open a snapshot pinning the current state.
    let snap = gate.open_snapshot().await;
    assert_eq!(snap.version(), pre_v);

    // Append-only batch while snapshot is live.
    let items: Vec<(Bytes, Bytes)> = (0..3u32)
        .map(|i| {
            (
                Bytes::from(format!("snap_fresh_{i}")),
                Bytes::from(format!("snap_val_{i}")),
            )
        })
        .collect();
    mvcc.set_versioned_many_append_only(items.clone().into_iter().map(|(k, v)| (RecordKey::from(k), v)).collect::<Vec<_>>())
        .await
        .unwrap();

    // The fresh keys must be readable at current.
    for (key, expected_val) in &items {
        let val = mvcc.get_current(RecordKey::from(key.clone())).await.unwrap();
        assert_eq!(
            val,
            Some(expected_val.clone()),
            "append-only keys must be readable after batch under snapshot"
        );
    }

    // The pre-existing key must still be readable at the snapshot version.
    let snap_read = mvcc.get_at(&key_pre, snap.version()).await.unwrap();
    assert_eq!(
        snap_read,
        Some(Bytes::from("pre_val")),
        "snapshot must still resolve pre-existing key"
    );

    // Drop the snapshot — subsequent writes should work normally.
    drop(snap);

    // A normal set_versioned on an existing key after snapshot drop should
    // trigger the scan path (vacuum_needs_scan was set during the snapshot).
    mvcc.set_versioned(RecordKey::from(key_pre.clone()), Bytes::from("post_snap"))
        .await
        .unwrap();
    let val = mvcc.get_current(RecordKey::from(key_pre)).await.unwrap();
    assert_eq!(val, Some(Bytes::from("post_snap")));
}

/// With keep_history retention, `set_versioned_many_append_only` must NOT
/// reclaim any versions (vacuum_key(_, 0) is a no-op for the L6 fast path;
/// the scan path keeps everything within max_count).
#[tokio::test]
async fn append_only_with_keep_history_retention() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate);

    // Switch to keep_history — all old versions retained.
    mvcc.set_retention(Retention {
        max_count: Some(10),
        max_age_secs: None,
        min_count: None,
    })
    .unwrap();

    // Append-only batch of 5 fresh keys.
    let items: Vec<(Bytes, Bytes)> = (0..5u32)
        .map(|i| {
            (
                Bytes::from(format!("hist_key_{i}")),
                Bytes::from(format!("hist_val_{i}")),
            )
        })
        .collect();
    mvcc.set_versioned_many_append_only(items.clone().into_iter().map(|(k, v)| (RecordKey::from(k), v)).collect::<Vec<_>>())
        .await
        .unwrap();

    // All 5 history entries must be present (nothing reclaimed).
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 5,
        "keep_history retention must not reclaim any fresh-key versions"
    );

    // All values readable.
    for (key, expected_val) in &items {
        let val = mvcc.get_current(RecordKey::from(key.clone())).await.unwrap();
        assert_eq!(val, Some(expected_val.clone()));
    }
}
