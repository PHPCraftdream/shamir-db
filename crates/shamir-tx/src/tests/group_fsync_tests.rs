use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};

use crate::group_fsync::GroupFsync;

type RecordStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

fn k(n: usize) -> RecordKey {
    Bytes::from(format!("key-{n}"))
}

fn v(n: usize) -> Bytes {
    Bytes::from(format!("value-{n}"))
}

/// Test fixture: wraps an `InMemoryStore`, counting `flush()` calls so a
/// test can assert that group-commit batching demonstrably amortised the
/// fsync. Everything else delegates straight to the inner store.
struct CountingStore {
    inner: InMemoryStore,
    flush_count: AtomicUsize,
}

impl CountingStore {
    fn new() -> Self {
        Self {
            inner: InMemoryStore::new(),
            flush_count: AtomicUsize::new(0),
        }
    }

    fn flush_count(&self) -> usize {
        self.flush_count.load(Ordering::Acquire)
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

    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        self.inner.set_many(items).await
    }

    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        self.inner.transact(ops).await
    }

    async fn flush(&self) -> DbResult<()> {
        self.flush_count.fetch_add(1, Ordering::AcqRel);
        self.inner.flush().await
    }

    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

#[tokio::test]
async fn single_append_is_durable() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let gf = GroupFsync::new(store.clone());

    gf.append_and_await(k(1), v(1)).await.unwrap();

    let got = store.get(k(1)).await.unwrap();
    assert_eq!(got, v(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_appends_all_durable() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let gf = Arc::new(GroupFsync::new(store.clone()));

    let mut handles = Vec::with_capacity(64);
    for n in 0..64 {
        let gf = gf.clone();
        handles.push(tokio::spawn(async move {
            gf.append_and_await(k(n), v(n)).await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }

    for n in 0..64 {
        let got = store.get(k(n)).await.unwrap();
        assert_eq!(got, v(n), "key {n} must round-trip with no cross-talk");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flush_is_amortized() {
    let counting = Arc::new(CountingStore::new());
    let store: Arc<dyn Store> = counting.clone();
    let gf = Arc::new(GroupFsync::new(store));

    let mut handles = Vec::with_capacity(64);
    for n in 0..64 {
        let gf = gf.clone();
        handles.push(tokio::spawn(async move {
            gf.append_and_await(k(n), v(n)).await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }

    let flushes = counting.flush_count();
    assert!(
        flushes >= 1,
        "at least one flush must have happened, got {flushes}"
    );
    assert!(
        flushes < 64,
        "batching must coalesce flushes below per-append, got {flushes}"
    );
}

#[tokio::test]
async fn distinct_values_roundtrip() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let gf = GroupFsync::new(store.clone());

    for n in 0..5 {
        gf.append_and_await(k(n), v(n * 100)).await.unwrap();
    }

    for n in 0..5 {
        let got = store.get(k(n)).await.unwrap();
        assert_eq!(got, v(n * 100));
    }
}
