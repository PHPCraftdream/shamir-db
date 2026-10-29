//! Regression test for `StoreChangelog::range_from` (task group 4,
//! 2026-08-14 cross-crate rush review): a small `limit` must not pay
//! O(N) work over the entire changelog journal.
//!
//! Proven with a counting `Store` wrapper that tallies every
//! `(key, value)` pair actually pulled off `iter_range_stream`'s
//! returned stream — i.e. what the caller consumed, not how many
//! records exist in the backing store.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};

use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::ChangelogStore as _;

use crate::repo::changelog_store::StoreChangelog;

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// Wraps an `Arc<dyn Store>` and counts every `(key, value)` pair yielded
/// by `iter_range_stream`'s returned stream as the caller drains it. All
/// other `Store` methods pass through uncounted.
struct RangeItemCountingStore {
    inner: Arc<dyn Store>,
    items_yielded: Arc<AtomicUsize>,
    batches: AtomicUsize,
    fail_batch_after_first: std::sync::atomic::AtomicBool,
}

impl RangeItemCountingStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            items_yielded: Arc::new(AtomicUsize::new(0)),
            batches: AtomicUsize::new(0),
            fail_batch_after_first: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn items_yielded(&self) -> usize {
        self.items_yielded.load(SeqCst)
    }
}

#[async_trait]
impl Store for RangeItemCountingStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        self.batches.fetch_add(1, SeqCst);
        if self.fail_batch_after_first.load(SeqCst) {
            if let Some((key, value)) = items.into_iter().next() {
                self.inner.set(key, value).await?;
            }
            return Err(DbError::Storage("injected partial batch".into()));
        }
        self.inner.set_many(items).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> TestStream {
        let inner_stream = self
            .inner
            .iter_range_stream(start_inclusive, end_inclusive, batch_size);
        let counter = Arc::clone(&self.items_yielded);
        Box::pin(inner_stream.inspect(move |batch| {
            if let Ok(items) = batch {
                counter.fetch_add(items.len(), SeqCst);
            }
        }))
    }
}

/// #1111-style regression: `range_from(from_key, limit=10)` against a
/// 5,000-entry changelog must stop shortly after collecting `limit`
/// candidates, not drain (and defensively sort) the entire journal tail.
/// Fails against the pre-fix implementation (which buffers all 5,000
/// entries before truncating).
#[tokio::test]
async fn range_from_does_not_buffer_the_whole_changelog_tail() {
    const TOTAL: u64 = 5_000;
    const LIMIT: usize = 10;

    let backing: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counting = Arc::new(RangeItemCountingStore::new(backing));
    let changelog = StoreChangelog::new(Arc::clone(&counting) as Arc<dyn Store>);

    for v in 0..TOTAL {
        let key = Bytes::copy_from_slice(&v.to_be_bytes());
        let value = Bytes::from(format!("event-{v}").into_bytes());
        changelog.put(key, value).await.unwrap();
    }

    let from_key = Bytes::copy_from_slice(&0u64.to_be_bytes());
    let results = changelog.range_from(from_key, LIMIT).await.unwrap();

    assert_eq!(results.len(), LIMIT);
    assert_eq!(results[0], Bytes::from_static(b"event-0"));
    assert_eq!(results[9], Bytes::from_static(b"event-9"));

    let yielded = counting.items_yielded();
    assert!(
        yielded < 200,
        "range_from(limit={LIMIT}) pulled {yielded} items out of a \
         {TOTAL}-entry changelog — it should stop shortly after `limit`, \
         not drain the whole journal tail"
    );
}

/// `limit = 0` must short-circuit to an empty result without reading
/// anything from the store.
#[tokio::test]
async fn range_from_zero_limit_reads_nothing() {
    let backing: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counting = Arc::new(RangeItemCountingStore::new(backing));
    let changelog = StoreChangelog::new(Arc::clone(&counting) as Arc<dyn Store>);

    changelog
        .put(
            Bytes::copy_from_slice(&0u64.to_be_bytes()),
            Bytes::from_static(b"event-0"),
        )
        .await
        .unwrap();

    let from_key = Bytes::copy_from_slice(&0u64.to_be_bytes());
    let results = changelog.range_from(from_key, 0).await.unwrap();

    assert!(results.is_empty());
    assert_eq!(counting.items_yielded(), 0);
}

#[tokio::test]
async fn rv_56651c4a_adapter_uses_storage_batch() {
    let backing: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counting = Arc::new(RangeItemCountingStore::new(backing));
    let adapter = StoreChangelog::new(counting.clone());
    let items: Vec<_> = (1u64..=3)
        .map(|v| {
            (
                Bytes::copy_from_slice(&v.to_be_bytes()),
                Bytes::from(vec![v as u8]),
            )
        })
        .collect();
    let results = adapter.put_many(items.clone()).await;
    assert_eq!(results.len(), 3);
    assert!(results.into_iter().all(|result| result.is_ok()));
    assert_eq!(counting.batches.load(SeqCst), 1);
    for (key, value) in items {
        assert_eq!(counting.get(key.into()).await.unwrap(), value);
    }
}

#[tokio::test]
async fn rv_56651c4a_adapter_unknown_partial_failure_is_conservative() {
    let counting = Arc::new(RangeItemCountingStore::new(Arc::new(InMemoryStore::new())));
    counting.fail_batch_after_first.store(true, SeqCst);
    let adapter = StoreChangelog::new(counting.clone());
    let items: Vec<_> = (1u64..=3)
        .map(|v| {
            (
                Bytes::copy_from_slice(&v.to_be_bytes()),
                Bytes::from_static(b"event"),
            )
        })
        .collect();
    let results = adapter.put_many(items.clone()).await;
    assert_eq!(results.len(), 3);
    assert!(results.into_iter().all(|result| result.is_err()));
    assert!(counting.get(items[0].0.clone().into()).await.is_ok());
    assert!(counting.get(items[1].0.clone().into()).await.is_err());
    assert_eq!(counting.batches.load(SeqCst), 1);
}
