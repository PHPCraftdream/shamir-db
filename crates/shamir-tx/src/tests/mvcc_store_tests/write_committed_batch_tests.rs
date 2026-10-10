//! L1 tests: verify `write_committed_batch_to_history` batches multiple
//! versions into a single `history.transact` call.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};

use crate::mvcc_store::{decode_ts_key, MvccStore};
use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::decode_version_key;

// ================================================================
// Counting Store — wraps InMemoryStore and counts `transact` calls.
// ================================================================

struct CountingStore {
    inner: InMemoryStore,
    transact_count: AtomicUsize,
}

impl CountingStore {
    fn new() -> Self {
        Self {
            inner: InMemoryStore::new(),
            transact_count: AtomicUsize::new(0),
        }
    }

    fn transact_count(&self) -> usize {
        self.transact_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Store for CountingStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        self.transact_count.fetch_add(1, Ordering::SeqCst);
        self.inner.transact(ops).await
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> std::pin::Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>
    {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> std::pin::Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>
    {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

fn make_counting_mvcc() -> (MvccStore, Arc<CountingStore>, Arc<RepoTxGate>) {
    let gate = Arc::new(RepoTxGate::fresh());
    let store = Arc::new(CountingStore::new());
    let mvcc = MvccStore::new(store.clone() as Arc<dyn Store>, gate.clone());
    (mvcc, store, gate)
}

/// Collect all (version, value) pairs for version-keys and all (version, ts)
/// pairs for ts-keys from history.
async fn scan_history(
    mvcc: &MvccStore,
) -> (
    Vec<(u64, Bytes)>, // version-keys: (version, value)
    Vec<(u64, u64)>,   // ts-keys: (version, ts_millis)
) {
    use futures::StreamExt;

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
// 1. Batch of 2+ versions x 2+ keys => ONE transact call
// =========================================================================
#[tokio::test]
async fn batch_multiple_versions_single_transact() {
    let (mvcc, store, gate) = make_counting_mvcc();
    let frozen_ts: u64 = 1_700_000_000_000;
    mvcc.set_test_now(frozen_ts);

    // Allocate versions via the gate (ascending).
    let v1 = gate.assign_next_version();
    let v2 = gate.assign_next_version();
    let v3 = gate.assign_next_version();

    // Simulate the ack-path stamping pending_ts.
    mvcc.apply_committed_visible(
        &[
            KvOp::Set(
                Bytes::from_static(b"k1").into(),
                Bytes::from_static(b"v1_1"),
            ),
            KvOp::Set(
                Bytes::from_static(b"k2").into(),
                Bytes::from_static(b"v2_1"),
            ),
        ],
        v1,
    );
    mvcc.apply_committed_visible(
        &[KvOp::Set(
            Bytes::from_static(b"k1").into(),
            Bytes::from_static(b"v1_2"),
        )],
        v2,
    );
    mvcc.apply_committed_visible(&[KvOp::Remove(Bytes::from_static(b"k2").into())], v3);

    assert_eq!(store.transact_count(), 0, "no transact before batch write");

    let pass: Vec<(u64, Vec<KvOp>)> = vec![
        (
            v1,
            vec![
                KvOp::Set(
                    Bytes::from_static(b"k1").into(),
                    Bytes::from_static(b"v1_1"),
                ),
                KvOp::Set(
                    Bytes::from_static(b"k2").into(),
                    Bytes::from_static(b"v2_1"),
                ),
            ],
        ),
        (
            v2,
            vec![KvOp::Set(
                Bytes::from_static(b"k1").into(),
                Bytes::from_static(b"v1_2"),
            )],
        ),
        (v3, vec![KvOp::Remove(Bytes::from_static(b"k2").into())]),
    ];

    mvcc.write_committed_batch_to_history(&pass).await.unwrap();

    // ONE transact call for all 3 versions.
    assert_eq!(
        store.transact_count(),
        1,
        "batch write must use exactly 1 transact"
    );

    // Verify data landed correctly.
    let (version_entries, ts_entries) = scan_history(&mvcc).await;

    // 3 data ops (k1@v1, k2@v1, k1@v2) + 1 tombstone (k2@v3) = 4 version-keys.
    assert_eq!(version_entries.len(), 4, "expected 4 version-key entries");

    // 3 ts entries (one per version).
    assert_eq!(ts_entries.len(), 3, "expected 3 ts entries");
    for (_, ts) in &ts_entries {
        assert_eq!(*ts, frozen_ts, "ts should match frozen clock");
    }

    // A14: pending_ts is read NON-DESTRUCTIVELY by the batched drain (so
    // multiple racing drains observe the same commit-time ts). The stamps
    // survive the drain and are reclaimed by `gc_overlay_to` once the
    // versions are durable.
    assert_eq!(
        mvcc.pending_ts_len(),
        3,
        "pending_ts stamps survive the batched drain (non-destructive read)"
    );
    mvcc.gate.mark_durable(v3);
    mvcc.gc_overlay_to(v3);
    assert_eq!(
        mvcc.pending_ts_len(),
        0,
        "pending_ts reclaimed by gc_overlay_to after the versions are durable"
    );
}

// =========================================================================
// 2. Empty batch is a no-op (no transact call)
// =========================================================================
#[tokio::test]
async fn batch_empty_is_noop() {
    let (mvcc, store, _gate) = make_counting_mvcc();

    mvcc.write_committed_batch_to_history(&[]).await.unwrap();

    assert_eq!(store.transact_count(), 0, "empty batch -> no transact");
}

// =========================================================================
// 3. Batch result matches N separate write_committed_to_history calls
// =========================================================================
#[tokio::test]
async fn batch_matches_per_version_write() {
    use futures::StreamExt;

    // Write via the batch method.
    let (mvcc_batch, _, gate_batch) = make_counting_mvcc();
    let frozen_ts: u64 = 1_700_000_000_000;
    mvcc_batch.set_test_now(frozen_ts);

    let v1 = gate_batch.assign_next_version();
    let v2 = gate_batch.assign_next_version();
    let ops1 = vec![KvOp::Set(
        Bytes::from_static(b"k1").into(),
        Bytes::from_static(b"val1"),
    )];
    let ops2 = vec![
        KvOp::Set(
            Bytes::from_static(b"k2").into(),
            Bytes::from_static(b"val2"),
        ),
        KvOp::Remove(Bytes::from_static(b"k1").into()),
    ];

    mvcc_batch.apply_committed_visible(&ops1, v1);
    mvcc_batch.apply_committed_visible(&ops2, v2);

    mvcc_batch
        .write_committed_batch_to_history(&[(v1, ops1.clone()), (v2, ops2.clone())])
        .await
        .unwrap();

    // Write via per-version method.
    let gate_single = Arc::new(RepoTxGate::fresh());
    let mvcc_single = MvccStore::new(Arc::new(InMemoryStore::new()), gate_single.clone());
    mvcc_single.set_test_now(frozen_ts);

    let sv1 = gate_single.assign_next_version();
    let sv2 = gate_single.assign_next_version();
    assert_eq!(sv1, v1, "same version allocation");
    assert_eq!(sv2, v2, "same version allocation");

    mvcc_single.apply_committed_visible(&ops1, sv1);
    mvcc_single.apply_committed_visible(&ops2, sv2);

    mvcc_single
        .write_committed_to_history(&ops1, sv1)
        .await
        .unwrap();
    mvcc_single
        .write_committed_to_history(&ops2, sv2)
        .await
        .unwrap();

    // Compare history contents.
    async fn collect_history(mvcc: &MvccStore) -> Vec<(Bytes, Bytes)> {
        let stream = mvcc.history_store().iter_stream(64);
        futures::pin_mut!(stream);
        let mut items: Vec<(Bytes, Bytes)> = Vec::new();
        while let Some(batch) = stream.next().await {
            for (k, v) in batch.unwrap() {
                items.push((k.into(), v));
            }
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));
        items
    }

    let batch_items = collect_history(&mvcc_batch).await;
    let single_items = collect_history(&mvcc_single).await;

    assert_eq!(
        batch_items.len(),
        single_items.len(),
        "same number of history entries"
    );
    for (b, s) in batch_items.iter().zip(single_items.iter()) {
        assert_eq!(b.0, s.0, "keys match");
        assert_eq!(b.1, s.1, "values match");
    }
}

// =========================================================================
// 4. publish_cell seeds correctly after batch (cold recovery path)
// =========================================================================
#[tokio::test]
async fn batch_seeds_cells_correctly() {
    let (mvcc, _, gate) = make_counting_mvcc();

    let v1 = gate.assign_next_version();
    let v2 = gate.assign_next_version();

    let pass = vec![
        (
            v1,
            vec![KvOp::Set(
                Bytes::from_static(b"k1").into(),
                Bytes::from_static(b"val1"),
            )],
        ),
        (
            v2,
            vec![KvOp::Set(
                Bytes::from_static(b"k1").into(),
                Bytes::from_static(b"val2"),
            )],
        ),
    ];

    mvcc.write_committed_batch_to_history(&pass).await.unwrap();

    // Cell should reflect the latest version (v2) for k1.
    assert_eq!(
        mvcc.version_of(b"k1"),
        v2,
        "cell should be seeded to latest version"
    );
}
