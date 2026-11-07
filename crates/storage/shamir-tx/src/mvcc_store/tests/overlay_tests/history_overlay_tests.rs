//! `history_of` must see versions that are committed and visible but not yet
//! drained into the `history` log.
//!
//! Since the D2 P1d-2b cutover the ack-path writes a commit only to the
//! in-memory overlay (`apply_committed_visible`); the background drainer lands
//! it in `history` later (`write_committed_to_history`). A timeline read in
//! that window used to miss the newest versions — an intermittent
//! `ts-e2e-nightly` failure (`history:*` e2e tests saw 1-2 of 3 versions).

use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::mvcc_store::ts_key;
use bytes::Bytes;
use futures::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};

use crate::mvcc_store::tests::gc::test_stores::counting_store::CountingStore;
use crate::mvcc_store::tests::gc::test_stores::failing_store::FailingStore;
use crate::mvcc_store::MvccStore;

use crate::mvcc_store::tests::gc::helpers::{make_gate, make_mvcc_with_gate};

fn set(key: &'static [u8], val: &'static [u8]) -> Vec<KvOp> {
    vec![KvOp::Set(
        Bytes::from_static(key).into(),
        Bytes::from_static(val),
    )]
}

struct KeyedRangeFailureStore(InMemoryStore);

#[async_trait::async_trait]
impl Store for KeyedRangeFailureStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.0.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.0.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.0.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.0.remove(key).await
    }

    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        self.0.transact(ops).await
    }

    fn iter_stream(
        &self,
        size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        self.0.iter_stream(size)
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        self.0.scan_prefix_stream(prefix, size)
    }

    fn iter_range_stream(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        _: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let message = if end.is_some() {
            "shared range"
        } else if start.as_ref().is_some_and(|key| key.starts_with(b"z\xff")) {
            "z error"
        } else {
            "a error"
        };
        Box::pin(futures::stream::once(async move {
            Err(DbError::Storage(message.into()))
        }))
    }
}

#[tokio::test]
async fn history_many_range_error_retries_in_input_order() {
    let history = Arc::new(KeyedRangeFailureStore(InMemoryStore::new()));
    let mvcc = MvccStore::new(history, make_gate());
    for (keys, expected) in [
        ([b"z".as_slice(), b"a".as_slice()], "z error"),
        ([b"a".as_slice(), b"z".as_slice()], "a error"),
    ] {
        let error = mvcc.history_of_many(&keys).await.unwrap_err();
        assert!(matches!(error, DbError::Storage(message) if message == expected));
    }
}

#[tokio::test]
async fn history_many_matches_single_and_uses_one_range() {
    let gate = make_gate();
    let history = Arc::new(CountingStore::new());
    let mvcc = MvccStore::new(history.clone(), gate.clone());
    for (key, value, ts) in [
        (b"k".as_slice(), b"a".as_slice(), 100),
        (b"kk", b"b", 200),
        (b"z", b"c", 300),
    ] {
        mvcc.set_test_now(ts);
        let v = gate.assign_next_version();
        let ops = set(key, value);
        mvcc.apply_committed_visible(&ops, v);
        mvcc.write_committed_to_history(&ops, v).await.unwrap();
    }
    mvcc.set_test_now(400);
    let v = gate.assign_next_version();
    mvcc.apply_committed_visible(&set(b"k", b"pending"), v);
    let keys: &[&[u8]] = &[b"z", b"k", b"missing", b"kk", b"k"];
    let mut expected = Vec::new();
    for key in keys {
        expected.push(mvcc.history_of(key).await.unwrap());
    }
    history.range_count.store(0, Ordering::Relaxed);
    assert_eq!(mvcc.history_of_many(keys).await.unwrap(), expected);
    assert_eq!(history.range_count.load(Ordering::Relaxed), 1);
    assert!(mvcc.history_of_many(&[]).await.unwrap().is_empty());
    assert_eq!(history.range_count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn history_many_recovers_scan_and_retries_timestamps() {
    let gate = make_gate();
    let history = Arc::new(FailingStore::new());
    let mvcc = MvccStore::new(history.clone(), gate.clone());
    mvcc.set_test_now(1234);
    let version = gate.assign_next_version();
    let ops = set(b"a", b"value");
    mvcc.apply_committed_visible(&ops, version);
    mvcc.write_committed_to_history(&ops, version)
        .await
        .unwrap();
    mvcc.gc_overlay_to(version);
    history.fail_scan.store(true, Ordering::Relaxed);
    let recovered = mvcc.history_of_many(&[b"z", b"a"]).await.unwrap();
    assert!(recovered[0].is_empty());
    assert_eq!(recovered[1][0].ts_millis, Some(1234));
    history.get_many_calls.store(0, Ordering::Relaxed);
    history.fail_many.store(true, Ordering::Relaxed);
    let result = mvcc.history_of_many(&[b"z", b"a"]).await.unwrap();
    assert!(result[0].is_empty());
    assert_eq!(result[1][0].ts_millis, Some(1234));
    assert_eq!(history.get_many_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn history_batches_archival_timestamps_and_preserves_pending() {
    let gate = make_gate();
    let history = Arc::new(FailingStore::new());
    let mvcc = MvccStore::new(history.clone(), gate.clone());
    mvcc.set_retention(crate::mvcc_store::Retention::keep_history())
        .unwrap();
    for ts in [1000, 2000] {
        mvcc.set_test_now(ts);
        mvcc.set_versioned(b"k".as_slice().into(), Bytes::from_static(b"archived"))
            .await
            .unwrap();
    }
    mvcc.set_test_now(3000);
    let version = gate.assign_next_version();
    mvcc.apply_committed_visible(&set(b"k", b"pending"), version);
    let timeline = mvcc.history_of(b"k").await.unwrap();
    assert_eq!(
        timeline.iter().map(|e| e.ts_millis).collect::<Vec<_>>(),
        vec![Some(1000), Some(2000), Some(3000)]
    );
    assert_eq!(history.get_many_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn rv_tx_3fc2fa1d_history_reporting_propagates_timestamp_storage_failure() {
    let gate = make_gate();
    let history = Arc::new(FailingStore::new());
    let mvcc = MvccStore::new(history.clone(), gate.clone());
    mvcc.set_test_now(1234);
    let version = gate.assign_next_version();
    let ops = set(b"reporting-error", b"value");
    mvcc.apply_committed_visible(&ops, version);
    mvcc.write_committed_to_history(&ops, version)
        .await
        .unwrap();
    mvcc.gc_overlay_to(version);

    history.fail_timestamp_get.store(true, Ordering::Relaxed);
    history.fail_many.store(true, Ordering::Relaxed);
    let history_single = mvcc.history_of(b"reporting-error").await;
    assert!(
        matches!(history_single, Err(DbError::Storage(message)) if message == "injected I/O fault")
    );
    assert_eq!(history.timestamp_get_calls.load(Ordering::Relaxed), 1);
    history.fail_timestamp_get.store(true, Ordering::Relaxed);
    history.fail_many.store(true, Ordering::Relaxed);
    let history_many = mvcc.history_of_many(&[b"reporting-error"]).await;
    assert!(
        matches!(history_many, Err(DbError::Storage(message)) if message == "injected I/O fault")
    );
    assert_eq!(history.timestamp_get_calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn rv_tx_3fc2fa1d_missing_timestamp_is_not_storage_failure() {
    let gate = make_gate();
    let history = Arc::new(FailingStore::new());
    let mvcc = MvccStore::new(history.clone(), gate.clone());
    let version = gate.assign_next_version();
    let ops = set(b"missing-ts", b"value");
    mvcc.apply_committed_visible(&ops, version);
    mvcc.write_committed_to_history(&ops, version)
        .await
        .unwrap();
    mvcc.gc_overlay_to(version);
    history.remove(ts_key(version).into()).await.unwrap();
    history.fail_many.store(true, Ordering::Relaxed);
    let missing_single = mvcc.history_of(b"missing-ts").await.unwrap();
    assert_eq!(missing_single[0].ts_millis, None);
    history.fail_many.store(true, Ordering::Relaxed);
    let missing_many = mvcc.history_of_many(&[b"missing-ts"]).await.unwrap();
    assert_eq!(missing_many[0][0].ts_millis, None);

    let valid_gate = make_gate();
    let valid_history = Arc::new(FailingStore::new());
    let valid_mvcc = MvccStore::new(valid_history, valid_gate.clone());
    let valid_version = valid_gate.assign_next_version();
    let valid_ops = set(b"present-ts", b"value");
    valid_mvcc.apply_committed_visible(&valid_ops, valid_version);
    valid_mvcc
        .write_committed_to_history(&valid_ops, valid_version)
        .await
        .unwrap();
    valid_mvcc.gc_overlay_to(valid_version);
    assert!(valid_mvcc.history_of(b"present-ts").await.unwrap()[0]
        .ts_millis
        .is_some());
}

/// Three commits acked through the overlay only (no drain yet): the whole
/// timeline must be visible, with each version's commit-time ts.
#[tokio::test]
async fn history_of_sees_overlay_only_versions() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    for (i, (val, ts)) in [(b"v1", 1_000u64), (b"v2", 2_000), (b"v3", 3_000)]
        .into_iter()
        .enumerate()
    {
        mvcc.set_test_now(ts);
        let v = gate.assign_next_version();
        assert_eq!(v, i as u64 + 1);
        mvcc.apply_committed_visible(&set(b"k", val), v);
    }

    let timeline = mvcc.history_of(b"k").await.unwrap();
    let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
    assert_eq!(versions, vec![1, 2, 3], "undrained versions must be listed");
    let values: Vec<&[u8]> = timeline.iter().map(|e| e.value.as_ref()).collect();
    assert_eq!(values, vec![b"v1".as_slice(), b"v2", b"v3"]);
    let stamps: Vec<Option<u64>> = timeline.iter().map(|e| e.ts_millis).collect();
    assert_eq!(
        stamps,
        vec![Some(1_000), Some(2_000), Some(3_000)],
        "commit-time ts must come from the pending stamp until drained"
    );
}

/// Drainer is partway through: v1/v2 already in `history`, v3 only in the
/// overlay (and v1/v2 still in the overlay too, GC hasn't run). The merge
/// must yield each version exactly once.
#[tokio::test]
async fn history_of_merges_overlay_and_history_without_duplicates() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let vals: [&'static [u8]; 3] = [b"v1", b"v2", b"v3"];
    let mut all_ops = Vec::new();
    for (i, val) in vals.into_iter().enumerate() {
        mvcc.set_test_now(1_000 * (i as u64 + 1));
        let v = gate.assign_next_version();
        let ops = set(b"k", val);
        mvcc.apply_committed_visible(&ops, v);
        all_ops.push((v, ops));
    }
    for (v, ops) in all_ops.iter().take(2) {
        mvcc.write_committed_to_history(ops, *v).await.unwrap();
    }

    let timeline = mvcc.history_of(b"k").await.unwrap();
    let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
    assert_eq!(versions, vec![1, 2, 3], "no duplicates, no gaps");
    assert_eq!(timeline[2].value.as_ref(), b"v3");
    assert_eq!(timeline[2].ts_millis, Some(3_000));
}

/// A delete acked through the overlay shows up as a tombstone (empty value),
/// the same convention `history` uses.
#[tokio::test]
async fn history_of_overlay_tombstone_is_empty_value() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let v1 = gate.assign_next_version();
    mvcc.apply_committed_visible(&set(b"k", b"alive"), v1);
    let v2 = gate.assign_next_version();
    mvcc.apply_committed_visible(&[KvOp::Remove(Bytes::from_static(b"k").into())], v2);

    let timeline = mvcc.history_of(b"k").await.unwrap();
    assert_eq!(timeline.len(), 2);
    assert_eq!(timeline[0].value.as_ref(), b"alive");
    assert!(timeline[1].value.is_empty(), "delete = tombstone");
}

/// Another key's overlay entries must not leak into this key's timeline.
#[tokio::test]
async fn history_of_overlay_is_scoped_to_the_key() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let v1 = gate.assign_next_version();
    mvcc.apply_committed_visible(&set(b"k", b"mine"), v1);
    let v2 = gate.assign_next_version();
    mvcc.apply_committed_visible(&set(b"k2", b"other"), v2);
    let v3 = gate.assign_next_version();
    mvcc.apply_committed_visible(&set(b"a", b"other"), v3);

    let timeline = mvcc.history_of(b"k").await.unwrap();
    assert_eq!(timeline.len(), 1);
    assert_eq!(timeline[0].version, v1);
}

#[tokio::test]
async fn history_timestamp_batch_failure_retries_and_keeps_unknown_age() {
    let gate = make_gate();
    let history = Arc::new(FailingStore::new());
    let mvcc = MvccStore::new(history.clone(), gate);
    mvcc.set_retention(crate::mvcc_store::Retention::keep_history())
        .unwrap();
    mvcc.set_test_now(1234);
    let v1 = mvcc
        .set_versioned(b"k".as_slice().into(), Bytes::from_static(b"a"))
        .await
        .unwrap();
    let v2 = mvcc
        .set_versioned(b"k".as_slice().into(), Bytes::from_static(b"b"))
        .await
        .unwrap();
    history.remove(ts_key(v2).into()).await.unwrap();
    history.fail_many.store(true, Ordering::Relaxed);
    let retried = mvcc.history_of(b"k").await.unwrap();
    assert_eq!(
        retried.iter().map(|e| e.ts_millis).collect::<Vec<_>>(),
        vec![Some(1234), None]
    );
    history.fail_many.store(false, Ordering::Relaxed);
    history.fail_get.store(true, Ordering::Relaxed);
    let failed = mvcc.history_of(b"k").await;
    assert!(
        matches!(failed, Err(DbError::Storage(message)) if message == "injected I/O fault"),
        "a failing individual timestamp retry is reported, not folded into unknown age"
    );
    assert_eq!(history.get_many_calls.load(Ordering::Relaxed), 2);
    history.fail_get.store(false, Ordering::Relaxed);
    let entries = mvcc.history_of(b"k").await.unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|e| (e.version, e.ts_millis))
            .collect::<Vec<_>>(),
        vec![(v1, Some(1234)), (v2, None)]
    );
}
