//! L2 ts-atomicity tests: verify that every write path folds ts into the
//! same `history.transact` batch as the data op — no separate ts write.

use bytes::Bytes;
use futures::StreamExt;

use crate::mvcc_store::{decode_ts_key, ts_key, MvccStore};
use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::decode_version_key;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::KvOp;
use shamir_storage::types::RecordKey;
use std::sync::Arc;

/// Build a test MvccStore with an in-memory history + shared gate.
fn make_mvcc() -> (MvccStore, Arc<RepoTxGate>) {
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = MvccStore::new(Arc::new(InMemoryStore::new()), gate.clone());
    (mvcc, gate)
}

/// Collect all (phys_key, value) pairs from the history store, returning
/// separate counts/data for version-keys and ts-keys.
async fn scan_history(
    mvcc: &MvccStore,
) -> (
    Vec<(u64, Bytes)>, // version-keys: (version, value)
    Vec<(u64, u64)>,   // ts-keys: (version, ts_millis)
) {
    let stream = mvcc.history_store().iter_stream(64);
    futures::pin_mut!(stream);
    let mut version_entries = Vec::new();
    let mut ts_entries = Vec::new();
    while let Some(batch) = stream.next().await {
        for (phys_key, val) in batch.unwrap() {
            if let Some((_orig, version)) = decode_version_key(&phys_key) {
                version_entries.push((version, val));
            } else if let Some(version) = decode_ts_key(&phys_key) {
                let ts_bytes: [u8; 8] = val.as_ref().try_into().unwrap();
                let ts_ms = u64::from_le_bytes(ts_bytes);
                ts_entries.push((version, ts_ms));
            }
        }
    }
    (version_entries, ts_entries)
}

// =========================================================================
// 1. set_versioned writes data + ts in one transaction
// =========================================================================
#[tokio::test]
async fn set_versioned_writes_ts_atomically() {
    let (mvcc, _gate) = make_mvcc();
    let frozen_ts: u64 = 1_700_000_000_000;
    mvcc.set_test_now(frozen_ts);

    let v = mvcc
        .set_versioned(
            RecordKey::from(Bytes::from_static(b"k1")),
            Bytes::from_static(b"v1"),
        )
        .await
        .unwrap();

    let (version_entries, ts_entries) = scan_history(&mvcc).await;
    // Exactly one version-key entry for version v.
    assert_eq!(version_entries.len(), 1, "expected 1 version-key");
    assert_eq!(version_entries[0].0, v);
    assert_eq!(version_entries[0].1.as_ref(), b"v1");

    // Exactly one ts-key entry for version v with the frozen clock value.
    assert_eq!(ts_entries.len(), 1, "expected 1 ts-key");
    assert_eq!(ts_entries[0].0, v);
    assert_eq!(ts_entries[0].1, frozen_ts);

    // Cross-check via direct history.get on the ts_key.
    let raw_ts = mvcc.history_store().get(ts_key(v).into()).await.unwrap();
    let ts_bytes: [u8; 8] = raw_ts.as_ref().try_into().unwrap();
    assert_eq!(u64::from_le_bytes(ts_bytes), frozen_ts);
}

// =========================================================================
// 2. set_versioned_many writes ts for every version in one transaction
// =========================================================================
#[tokio::test]
async fn set_versioned_many_writes_ts_atomically() {
    let (mvcc, _gate) = make_mvcc();
    let frozen_ts: u64 = 1_700_000_000_000;
    mvcc.set_test_now(frozen_ts);

    let items: Vec<(Bytes, Bytes)> = (0..3u8)
        .map(|i| (Bytes::from(vec![b'k', i]), Bytes::from(vec![b'v', i])))
        .collect();

    let max_v = mvcc
        .set_versioned_many(
            items
                .into_iter()
                .map(|(k, v)| (RecordKey::from(k), v))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    assert!(max_v >= 3, "expected at least 3 versions allocated");

    let (version_entries, ts_entries) = scan_history(&mvcc).await;
    // 3 version-keys + 3 ts-keys.
    assert_eq!(version_entries.len(), 3, "expected 3 version-keys");
    assert_eq!(ts_entries.len(), 3, "expected 3 ts-keys");

    // Every ts entry has the frozen clock value.
    for (_, ts_ms) in &ts_entries {
        assert_eq!(*ts_ms, frozen_ts);
    }

    // Every version that has a data entry also has a ts entry.
    let ts_versions: shamir_collections::TFxSet<u64> = ts_entries.iter().map(|(v, _)| *v).collect();
    for (v, _) in &version_entries {
        assert!(
            ts_versions.contains(v),
            "version {} has data but no ts entry",
            v
        );
    }
}

// =========================================================================
// 3. delete_versioned writes tombstone-data + ts atomically
//
// Note: default retention is CurrentOnly (`max_count: Some(0)`), so the
// pre-delete version v1 is reclaimed by `vacuum_key` after the delete. To
// observe BOTH the insert and the tombstone in the same scan we install
// KeepHistory retention. This isolates the L2 atomicity invariant (ts rides
// in the same `transact` as data) from the orthogonal vacuum policy.
// =========================================================================
#[tokio::test]
async fn delete_versioned_writes_ts_atomically() {
    use crate::mvcc_store::Retention;

    let (mvcc, _gate) = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();
    let frozen_ts: u64 = 1_700_000_000_000;
    mvcc.set_test_now(frozen_ts);

    // First insert so there's something to delete.
    let _v1 = mvcc
        .set_versioned(
            RecordKey::from(Bytes::from_static(b"dk")),
            Bytes::from_static(b"dv"),
        )
        .await
        .unwrap();

    let v2 = mvcc
        .delete_versioned(RecordKey::from(Bytes::from_static(b"dk")))
        .await
        .unwrap();

    let (version_entries, ts_entries) = scan_history(&mvcc).await;
    // 2 version-keys (insert + tombstone), 2 ts-keys.
    assert_eq!(version_entries.len(), 2);
    assert_eq!(ts_entries.len(), 2);

    // The tombstone entry has empty value.
    let tombstone = version_entries.iter().find(|(v, _)| *v == v2).unwrap();
    assert!(tombstone.1.is_empty(), "tombstone should be empty bytes");

    // ts for the delete version exists and is correct.
    let ts_for_delete = ts_entries.iter().find(|(v, _)| *v == v2).unwrap();
    assert_eq!(ts_for_delete.1, frozen_ts);
}

// =========================================================================
// 4. write_committed_to_history (drain path) — data + ts atomic
// =========================================================================
#[tokio::test]
async fn write_committed_to_history_ts_atomic() {
    let (mvcc, gate) = make_mvcc();
    let frozen_ts: u64 = 1_700_000_000_000;
    mvcc.set_test_now(frozen_ts);

    let commit_version = gate.assign_next_version();
    let ops = vec![
        KvOp::Set(
            Bytes::from_static(b"hk1").into(),
            Bytes::from_static(b"hv1"),
        ),
        KvOp::Set(
            Bytes::from_static(b"hk2").into(),
            Bytes::from_static(b"hv2"),
        ),
    ];

    // Simulate the ack path stamping the pending ts.
    mvcc.apply_committed_visible(&ops, commit_version);

    // Drain path writes history.
    mvcc.write_committed_to_history(&ops, commit_version)
        .await
        .unwrap();

    let (version_entries, ts_entries) = scan_history(&mvcc).await;
    // 2 data entries (hk1, hk2) + 1 ts entry for commit_version.
    assert_eq!(version_entries.len(), 2, "expected 2 version-keys");
    assert_eq!(
        ts_entries.len(),
        1,
        "expected 1 ts-key for the commit version"
    );
    assert_eq!(ts_entries[0].0, commit_version);
    assert_eq!(ts_entries[0].1, frozen_ts);

    // A14: pending_ts is read NON-DESTRUCTIVELY by the drain (so multiple
    // racing drains observe the same commit-time ts). The stamp survives
    // the drain and is reclaimed by `gc_overlay_to` once the version is
    // durable. Pre-A14 the drain itself removed the stamp; that lost it
    // for any second racer.
    assert_eq!(
        mvcc.pending_ts_len(),
        1,
        "pending_ts stamp survives the drain (non-destructive read)"
    );
    mvcc.gate.mark_durable(commit_version);
    mvcc.gc_overlay_to(commit_version);
    assert_eq!(
        mvcc.pending_ts_len(),
        0,
        "pending_ts reclaimed by gc_overlay_to after the version is durable"
    );
}

// =========================================================================
// 5. Atomicity on error: transact failure leaves neither data nor ts
// =========================================================================
#[tokio::test]
async fn transact_failure_leaves_no_data_no_ts() {
    use super::test_stores::make_failing_history_mvcc;

    let gate = Arc::new(RepoTxGate::fresh());
    let (mvcc, failing) = make_failing_history_mvcc(gate);
    let frozen_ts: u64 = 1_700_000_000_000;
    mvcc.set_test_now(frozen_ts);

    // Arm the fault — `set` inside `transact` will fail.
    failing
        .fail_set
        .store(true, std::sync::atomic::Ordering::Relaxed);

    let result = mvcc
        .set_versioned(
            RecordKey::from(Bytes::from_static(b"fk")),
            Bytes::from_static(b"fv"),
        )
        .await;

    assert!(
        result.is_err(),
        "set_versioned should propagate transact error"
    );

    // Neither data nor ts should be in history (the transact failed atomically).
    let (version_entries, ts_entries) = scan_history(&mvcc).await;
    assert_eq!(
        version_entries.len(),
        0,
        "no data should be written on transact failure"
    );
    assert_eq!(
        ts_entries.len(),
        0,
        "no ts should be written on transact failure"
    );
}
