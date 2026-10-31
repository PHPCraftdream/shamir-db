#![allow(deprecated)]

use crate::error::DbResult;
use crate::storage_in_memory::InMemoryRepo;
use crate::storage_membuffer::{MemBufferConfig, MemBufferStore};
use crate::types::{KvOp, RecordKey, RecordStream, Repo, Store};
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::{Arc, Mutex};
// Audit §2.3 regression: `transact` must NOT lose a concurrent `set(k)`
// that lands between `inner.transact` and the post-transact `dirty.remove`.
// Before the fix, the unconditional `dirty.remove(&k)` deleted a concurrent
// write's dirty entry, so it never reached the inner store — after a cache
// eviction or restart, durable state was the OLD value.
//
// This test deterministically reproduces the race: a wrapper inner store
// injects a `set(k, "concurrent")` on the SAME `MemBufferStore` (via a
// late-bound slot) during its `transact` call — landing the concurrent write
// in dirty exactly in the window the old code lost. After `transact` + flush,
// the concurrent value must survive in inner (the old unconditional remove
// would have deleted it, losing the write permanently).

/// Wrapper inner store that, during `transact`, injects a concurrent
/// `set` on the `MemBufferStore` that wraps it (via a late-bound slot).
struct ConcurrentWriterInner {
    inner: Arc<dyn Store>,
    /// Late-bound reference to the MemBufferStore that wraps this inner.
    /// Set after construction so the wrapper can inject a concurrent
    /// write through the SAME dirty map the wrapping transact operates on.
    buffered_slot: Mutex<Option<Arc<MemBufferStore>>>,
    inject_key: RecordKey,
    inject_value: Bytes,
}

#[async_trait]
impl Store for ConcurrentWriterInner {
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
    async fn transact(&self, _ops: Vec<KvOp>) -> DbResult<()> {
        // The wrapping MemBufferStore called `inner.transact`. Inject a
        // concurrent `set` through the buffered handle so it lands in the
        // SAME dirty map — exactly the race window the old code lost.
        // Clone the Arc out of the Mutex BEFORE awaiting so the guard
        // is not held across the `.await` (Send requirement).
        let buffered_opt = self.buffered_slot.lock().unwrap().clone();
        if let Some(buffered) = buffered_opt {
            buffered
                .set(self.inject_key.clone(), self.inject_value.clone())
                .await?;
        }
        Ok(())
    }
    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    fn iter_range_stream(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        self.inner.iter_range_stream(start, end, batch_size)
    }
    fn iter_range_stream_reverse(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        self.inner.iter_range_stream_reverse(start, end, batch_size)
    }
}

#[tokio::test]
async fn transact_does_not_lose_concurrent_set() {
    let inner_repo = InMemoryRepo::new();
    let real_inner: Arc<dyn Store> = inner_repo.store_get("t").await.unwrap();

    let key = RecordKey::from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    let inject_value = Bytes::from_static(b"concurrent");

    let wrapper = Arc::new(ConcurrentWriterInner {
        inner: real_inner.clone(),
        buffered_slot: Mutex::new(None),
        inject_key: key.clone(),
        inject_value: inject_value.clone(),
    });
    let wrapper_dyn: Arc<dyn Store> = wrapper.clone();
    let buffered = Arc::new(MemBufferStore::new(wrapper_dyn, MemBufferConfig::default()));
    // Late-bind: now the wrapper can inject through `buffered`.
    *wrapper.buffered_slot.lock().unwrap() = Some(Arc::clone(&buffered));

    // Call transact on the key. The wrapper's transact injects a
    // concurrent set("concurrent") into dirty mid-transact.
    buffered
        .transact(vec![KvOp::Set(
            key.clone(),
            Bytes::from_static(b"transacted"),
        )])
        .await
        .unwrap();

    // Flush so dirty reaches inner.
    buffered.flush().await.unwrap();

    // The concurrent value ("concurrent") must survive — the old
    // unconditional `dirty.remove(&k)` would have deleted it, leaving
    // inner with "transacted" (the stale value) instead.
    let got = real_inner.get(key.clone()).await.unwrap();
    assert_eq!(
        got.as_ref(),
        b"concurrent",
        "concurrent set must NOT be lost by transact's dirty cleanup (audit §2.3); \
             got {:?}",
        got.as_ref()
    );
}
