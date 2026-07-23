use bytes::Bytes;
use shamir_storage::types::RecordKey;

use super::helpers::make_mvcc;

/// Helper: call `get_at_many` and `get_at` per-key, assert equal.
async fn assert_many_matches_individual(
    mvcc: &crate::mvcc_store::MvccStore,
    keys: &[Bytes],
    snapshot_version: u64,
) {
    let batch = mvcc.get_at_many(keys, snapshot_version).await.unwrap();
    assert_eq!(batch.len(), keys.len(), "result length must match input");
    for (i, key) in keys.iter().enumerate() {
        let single = mvcc.get_at(key, snapshot_version).await.unwrap();
        assert_eq!(batch[i], single, "mismatch at index {i} for key {:?}", key);
    }
}

// ---- mixed batch, all direct-path ----

#[tokio::test]
async fn get_at_many_all_direct_path() {
    let mvcc = make_mvcc();
    let k1 = Bytes::from_static(b"a");
    let k2 = Bytes::from_static(b"b");
    let k3 = Bytes::from_static(b"c");

    mvcc.set_versioned(RecordKey::from(k1.clone()), Bytes::from_static(b"v1"))
        .await
        .unwrap();
    mvcc.set_versioned(RecordKey::from(k2.clone()), Bytes::from_static(b"v2"))
        .await
        .unwrap();
    let snapshot = mvcc
        .set_versioned(RecordKey::from(k3.clone()), Bytes::from_static(b"v3"))
        .await
        .unwrap();

    let keys = vec![k1.clone(), k2.clone(), k3.clone()];
    let result = mvcc.get_at_many(&keys, snapshot).await.unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0], Some(Bytes::from_static(b"v1")));
    assert_eq!(result[1], Some(Bytes::from_static(b"v2")));
    assert_eq!(result[2], Some(Bytes::from_static(b"v3")));

    // Cross-check against per-key get_at.
    assert_many_matches_individual(&mvcc, &keys, snapshot).await;
}

// ---- missing key in the batch does not corrupt/misalign the rest ----

#[tokio::test]
async fn get_at_many_missing_key_does_not_misalign() {
    let mvcc = make_mvcc();
    let k1 = Bytes::from_static(b"present1");
    let k_absent = Bytes::from_static(b"never_written");
    let k2 = Bytes::from_static(b"present2");

    mvcc.set_versioned(RecordKey::from(k1.clone()), Bytes::from_static(b"v1"))
        .await
        .unwrap();
    let snapshot = mvcc
        .set_versioned(RecordKey::from(k2.clone()), Bytes::from_static(b"v2"))
        .await
        .unwrap();

    let keys = vec![k1.clone(), k_absent.clone(), k2.clone()];
    let result = mvcc.get_at_many(&keys, snapshot).await.unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0], Some(Bytes::from_static(b"v1")));
    assert_eq!(result[1], None, "never-written key must resolve to None");
    assert_eq!(result[2], Some(Bytes::from_static(b"v2")));

    assert_many_matches_individual(&mvcc, &keys, snapshot).await;
}

// ---- tombstone in the batch resolves to None, matching get_at ----

#[tokio::test]
async fn get_at_many_tombstone_resolves_to_none() {
    let mvcc = make_mvcc();
    let k_tomb = Bytes::from_static(b"deleted");
    let k_alive = Bytes::from_static(b"alive");

    mvcc.set_versioned(
        RecordKey::from(k_tomb.clone()),
        Bytes::from_static(b"was_here"),
    )
    .await
    .unwrap();
    // Delete it — tombstone (empty bytes) written at a later version.
    mvcc.delete_versioned(RecordKey::from(k_tomb.clone()))
        .await
        .unwrap();
    let snapshot = mvcc
        .set_versioned(
            RecordKey::from(k_alive.clone()),
            Bytes::from_static(b"still_here"),
        )
        .await
        .unwrap();

    let keys = vec![k_tomb.clone(), k_alive.clone()];
    let result = mvcc.get_at_many(&keys, snapshot).await.unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(
        result[0], None,
        "tombstoned key (deleted, still alive at snapshot's cell state) must resolve to None"
    );
    assert_eq!(result[1], Some(Bytes::from_static(b"still_here")));

    assert_many_matches_individual(&mvcc, &keys, snapshot).await;
}

// ---- mixed direct-path + fallback-path in the SAME batch ----

#[tokio::test]
async fn get_at_many_mixed_direct_and_fallback_path() {
    let mvcc = make_mvcc();
    let k_direct = Bytes::from_static(b"direct_key");
    let k_concurrent = Bytes::from_static(b"concurrent_key");

    // Write both keys once — this establishes the snapshot version.
    mvcc.set_versioned(
        RecordKey::from(k_direct.clone()),
        Bytes::from_static(b"d_v1"),
    )
    .await
    .unwrap();
    let snapshot = mvcc
        .set_versioned(
            RecordKey::from(k_concurrent.clone()),
            Bytes::from_static(b"c_v1"),
        )
        .await
        .unwrap();

    // A "concurrent" write lands on k_concurrent AFTER the snapshot was
    // pinned — its cell's cur_v now exceeds `snapshot`, forcing the
    // fallback range-scan path for that key while k_direct stays direct.
    mvcc.set_versioned(
        RecordKey::from(k_concurrent.clone()),
        Bytes::from_static(b"c_v2_after_snapshot"),
    )
    .await
    .unwrap();

    let keys = vec![k_direct.clone(), k_concurrent.clone()];
    let result = mvcc.get_at_many(&keys, snapshot).await.unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(
        result[0],
        Some(Bytes::from_static(b"d_v1")),
        "direct-path key resolves via the batched history.get_many subset"
    );
    assert_eq!(
        result[1],
        Some(Bytes::from_static(b"c_v1")),
        "fallback-path key must see the pre-snapshot value, not the concurrent write"
    );

    // Cross-check: identical to per-key get_at at the same snapshot.
    assert_many_matches_individual(&mvcc, &keys, snapshot).await;
}

// ---- empty input: no I/O, returns Ok(vec![]) ----

#[tokio::test]
async fn get_at_many_empty_input() {
    let mvcc = make_mvcc();
    let result = mvcc.get_at_many(&[], 0).await.unwrap();
    assert!(result.is_empty());
}
