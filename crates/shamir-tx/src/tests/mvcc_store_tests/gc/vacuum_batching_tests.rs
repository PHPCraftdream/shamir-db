//! RV-TX-088DE4B5 — vacuum reclaim I/O must be batched.
//!
//! `vacuum_key` used to issue two individual `remove_no_flag` round-trips
//! per reclaimed version (and looked the commit timestamp up twice on the
//! age-capped scan path). The fix batches the data+ts removals of a vacuum
//! pass into ONE `history.transact` and reuses the age-check lookup for the
//! ts-index prune.
//!
//! These tests count individual `remove_no_flag` calls vs `Remove` ops
//! delivered through `transact` on a counting double — deterministic, no
//! timing. On the unfixed code `remove_no_flag` is called directly and the
//! transact path never sees a `Remove`, so both assertions fail.

use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};

use super::helpers::{count_history_entries, make_gate};
use crate::mvcc_store::MvccStore;
use crate::mvcc_store::Retention;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::stream::Stream;

/// Counts `remove_no_flag` calls, `Remove` ops and invocations of
/// `transact`, and ts-key `get` reads (ts-key shape: 9 bytes, first byte
/// `TS_TAG = 0x00` — the encoding `crate::mvcc_store::ts_key` produces).
struct IoCountingStore {
    inner: InMemoryStore,
    remove_no_flag_calls: AtomicUsize,
    transact_remove_ops: AtomicUsize,
    /// Incremented once per `transact` invocation, regardless of op count.
    transact_calls: AtomicUsize,
    /// Incremented in `get` when the key is a 9-byte ts key (TS_TAG 0x00).
    ts_reads: AtomicUsize,
}

impl IoCountingStore {
    fn new() -> Self {
        Self {
            inner: InMemoryStore::new(),
            remove_no_flag_calls: AtomicUsize::new(0),
            transact_remove_ops: AtomicUsize::new(0),
            transact_calls: AtomicUsize::new(0),
            ts_reads: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl Store for IoCountingStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        // ts-key shape: [TS_TAG = 0x00][8 bytes version_be] = 9 bytes.
        if key.len() == 9 && key.as_ref()[0] == 0x00 {
            self.ts_reads.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }
    async fn remove_no_flag(&self, key: RecordKey) -> DbResult<()> {
        self.remove_no_flag_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.remove_no_flag(key).await
    }
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        self.transact_calls.fetch_add(1, Ordering::SeqCst);
        for op in &ops {
            if matches!(op, KvOp::Remove(_)) {
                self.transact_remove_ops.fetch_add(1, Ordering::SeqCst);
            }
        }
        self.inner.transact(ops).await
    }
    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

fn make_counting_mvcc(store: Arc<IoCountingStore>) -> MvccStore {
    MvccStore::new(store as Arc<dyn Store>, make_gate())
}

/// RV-TX-088DE4B5 — age-capped scan reclaim: the reclaimed version's data
/// key and ts key must be removed in ONE batched `transact` (2 Remove ops,
/// zero direct `remove_no_flag` calls).
#[tokio::test]
async fn rv_tx_088de4b5_age_scan_reclaim_batches_removals_into_one_transact() {
    let history = Arc::new(IoCountingStore::new());
    let mvcc = make_counting_mvcc(history.clone());
    mvcc.set_retention(Retention {
        max_age_secs: Some(60),
        max_count: None,
        min_count: None,
    })
    .unwrap();

    let key = RecordKey::from(Bytes::from_static(b"age_key"));
    mvcc.set_test_now(1);
    mvcc.set_versioned(key.clone(), Bytes::from_static(b"old"))
        .await
        .unwrap();

    // 100s later — the vacuum triggered by this write reclaims v1 by age.
    mvcc.set_test_now(100_001);
    mvcc.set_versioned(key.clone(), Bytes::from_static(b"new1"))
        .await
        .unwrap();

    // w1 and w2 each issue ONE write transact; w2's vacuum adds exactly ONE
    // batched reclaim transact (3 total). A split into two one-op transacts
    // would make this 4.
    assert_eq!(
        history.transact_calls.load(Ordering::SeqCst),
        3,
        "w1 + w2 writes + ONE batched reclaim transact = 3"
    );
    // Exactly ONE ts read for the reclaimed version (the age check, reused
    // for the prune); the pre-fix duplicate lookup for the index prune
    // would push this to 2.
    assert_eq!(
        history.ts_reads.load(Ordering::SeqCst),
        1,
        "the reclaimed version must be looked up exactly once (age check \
         reused for the prune)"
    );

    // w3: one write transact; its vacuum age-checks v2 (within the window →
    // keep, one more ts read) and reclaims nothing.
    mvcc.set_versioned(key.clone(), Bytes::from_static(b"new2"))
        .await
        .unwrap();

    // The reclaim actually happened: v1 gone, v2 (age-kept) + v3 (current).
    assert_eq!(count_history_entries(&mvcc).await, 2);
    assert_eq!(
        history.transact_calls.load(Ordering::SeqCst),
        4,
        "w3 adds exactly its own write transact (no reclaim work)"
    );
    assert_eq!(
        history.ts_reads.load(Ordering::SeqCst),
        2,
        "v2's age check at w3 is the only further ts read"
    );

    assert_eq!(
        history.transact_remove_ops.load(Ordering::SeqCst),
        2,
        "the reclaimed version's data+ts keys must be removed via ONE batched \
         transact carrying 2 Remove ops"
    );
    assert_eq!(
        history.remove_no_flag_calls.load(Ordering::SeqCst),
        0,
        "vacuum must not issue individual remove_no_flag round-trips"
    );
}

/// RV-TX-088DE4B5 — CurrentOnly fast path: the previously-deferred anchor
/// version must be deleted via one batched transact (2 Remove ops), not two
/// individual `remove_no_flag` calls.
#[tokio::test]
async fn rv_tx_088de4b5_fast_path_deferred_anchor_removal_is_batched() {
    let history = Arc::new(IoCountingStore::new());
    let mvcc = make_counting_mvcc(history.clone());

    let key = RecordKey::from(Bytes::from_static(b"fast_key"));
    // w1: append-only, nothing to defer. w2: defers v1 as anchor.
    // w3: deletes the previously-deferred v1 (data + ts keys).
    for i in 1..=3u8 {
        mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    // Current + deferred anchor survive.
    assert_eq!(count_history_entries(&mvcc).await, 2);

    // 3 write transacts + exactly ONE batched reclaim transact (a split
    // into two one-op transacts would make this 5).
    assert_eq!(
        history.transact_calls.load(Ordering::SeqCst),
        4,
        "3 writes + ONE batched deferred-anchor reclaim transact = 4"
    );
    assert_eq!(
        history.ts_reads.load(Ordering::SeqCst),
        1,
        "the deferred anchor's commit ts must be read exactly once"
    );

    assert_eq!(
        history.transact_remove_ops.load(Ordering::SeqCst),
        2,
        "the deferred anchor's data+ts keys must be removed via ONE batched \
         transact carrying 2 Remove ops"
    );
    assert_eq!(
        history.remove_no_flag_calls.load(Ordering::SeqCst),
        0,
        "the fast path must not issue individual remove_no_flag round-trips"
    );
}
