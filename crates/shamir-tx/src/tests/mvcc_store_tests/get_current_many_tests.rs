use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;

use super::helpers::{make_gate, make_mvcc, make_mvcc_with_gate};

/// Helper: call get_current_many and get_current_bytes per-key, assert equal.
async fn assert_many_matches_individual(mvcc: &crate::mvcc_store::MvccStore, keys: &[Bytes]) {
    let batch = mvcc.get_current_many(keys).await.unwrap();
    assert_eq!(batch.len(), keys.len(), "result length must match input");
    for (i, key) in keys.iter().enumerate() {
        let single = mvcc.get_current_bytes(key).await.unwrap();
        assert_eq!(batch[i], single, "mismatch at index {i} for key {:?}", key);
    }
}

// ---- basic: warm / absent / tombstone mix ----

#[tokio::test]
async fn get_current_many_warm_absent_tombstone() {
    let mvcc = make_mvcc();
    let k1 = Bytes::from_static(b"a");
    let k2 = Bytes::from_static(b"b");
    let k3 = Bytes::from_static(b"c");
    let k_absent = Bytes::from_static(b"absent");

    // Write k1, k2, k3.
    mvcc.set_versioned(k1.clone(), Bytes::from_static(b"v1"))
        .await
        .unwrap();
    mvcc.set_versioned(k2.clone(), Bytes::from_static(b"v2"))
        .await
        .unwrap();
    mvcc.set_versioned(k3.clone(), Bytes::from_static(b"v3"))
        .await
        .unwrap();

    // Delete k2 (tombstone).
    mvcc.delete_versioned(k2.clone()).await.unwrap();

    let keys = vec![k1.clone(), k2.clone(), k3.clone(), k_absent.clone()];
    let result = mvcc.get_current_many(&keys).await.unwrap();
    assert_eq!(result.len(), 4);
    assert_eq!(result[0], Some(Bytes::from_static(b"v1")));
    assert_eq!(result[1], None); // tombstone
    assert_eq!(result[2], Some(Bytes::from_static(b"v3")));
    assert_eq!(result[3], None); // absent

    // Cross-check against per-key reads.
    assert_many_matches_individual(&mvcc, &keys).await;
}

// ---- result order matches input order ----

#[tokio::test]
async fn get_current_many_preserves_order() {
    let mvcc = make_mvcc();
    let keys_data: Vec<(Bytes, Bytes)> = (0u8..10)
        .map(|i| (Bytes::from(vec![b'k', i]), Bytes::from(vec![b'v', i])))
        .collect();
    for (k, v) in &keys_data {
        mvcc.set_versioned(k.clone(), v.clone()).await.unwrap();
    }

    // Read in reverse order.
    let rev_keys: Vec<Bytes> = keys_data.iter().rev().map(|(k, _)| k.clone()).collect();
    let result = mvcc.get_current_many(&rev_keys).await.unwrap();
    for (i, (k, v)) in keys_data.iter().rev().enumerate() {
        assert_eq!(
            result[i],
            Some(v.clone()),
            "order mismatch at position {i} for key {:?}",
            k
        );
    }
}

// ---- empty input ----

#[tokio::test]
async fn get_current_many_empty() {
    let mvcc = make_mvcc();
    let result = mvcc.get_current_many(&[]).await.unwrap();
    assert!(result.is_empty());
}

// ---- R3 floor-cap: version above floor reads as snapshot ----

#[tokio::test]
async fn get_current_many_floor_cap() {
    // Use a gate where we can control the committed floor.
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let k = Bytes::from_static(b"floor_key");

    // Write v1 at version 1.
    mvcc.set_versioned(k.clone(), Bytes::from_static(b"val1"))
        .await
        .unwrap();
    // Write v2 at version 2.
    mvcc.set_versioned(k.clone(), Bytes::from_static(b"val2"))
        .await
        .unwrap();

    // The floor (last_committed) is now at 2 (both writes committed).
    // get_current_many should return val2.
    let result = mvcc
        .get_current_many(std::slice::from_ref(&k))
        .await
        .unwrap();
    assert_eq!(result[0], Some(Bytes::from_static(b"val2")));

    // Cross-check.
    assert_many_matches_individual(&mvcc, std::slice::from_ref(&k)).await;
}

// ---- cold-start fallback works through get_current_many ----

#[tokio::test]
async fn get_current_many_cold_start() {
    let gate = make_gate();
    let history = Arc::new(InMemoryStore::new());

    // Pre-populate history with a version-key entry (simulating prior writes
    // whose cell was evicted).
    let vk = crate::version_codec::encode_version_key(b"cold", 1);
    history
        .set(vk, Bytes::from_static(b"cold_val"))
        .await
        .unwrap();
    // Also write the ts entry so the gate doesn't trip.
    let ts = crate::mvcc_store::ts_key(1);
    history
        .set(ts, Bytes::from(1u64.to_le_bytes().to_vec()))
        .await
        .unwrap();

    let mvcc = crate::mvcc_store::MvccStore::new(history as Arc<dyn Store>, gate);
    // The cell for "cold" is absent → current_version == 0 → cold path.
    let k = Bytes::from_static(b"cold");
    let result = mvcc
        .get_current_many(std::slice::from_ref(&k))
        .await
        .unwrap();
    assert_eq!(result[0], Some(Bytes::from_static(b"cold_val")));

    // Cross-check.
    assert_many_matches_individual(&mvcc, &[k]).await;
}

// ---- tombstone in warm miss-set ----

#[tokio::test]
async fn get_current_many_tombstone_in_miss_set() {
    let mvcc = make_mvcc();
    let k = Bytes::from_static(b"tomb");

    // Write then delete.
    mvcc.set_versioned(k.clone(), Bytes::from_static(b"alive"))
        .await
        .unwrap();
    mvcc.delete_versioned(k.clone()).await.unwrap();

    let result = mvcc
        .get_current_many(std::slice::from_ref(&k))
        .await
        .unwrap();
    assert_eq!(result[0], None);

    // Cross-check.
    assert_many_matches_individual(&mvcc, std::slice::from_ref(&k)).await;
}
