//! `MemBufferStore` — bounded LRU + write-back cache wrapper over
//! any `Store`. Currently a **PASSTHROUGH PROXY**: every method
//! delegates directly to the inner store. This is the baseline
//! shape against which the real buffered implementation will be
//! benched.
//!
//! # Goal of the buffered version (not yet implemented)
//!
//! - Hold a bounded LRU cache of recently-touched records in memory.
//! - Reads served from cache; misses fall through to inner store
//!   and populate the cache (evicting cold entries if at capacity).
//! - Writes land in cache + dirty queue and return immediately.
//!   A background flusher drains the dirty queue to the inner store
//!   in batches (using `set_many` / `remove_many`).
//! - All latency-critical paths happen in memory; disk work runs
//!   asynchronously, sequentially, with bounded back-pressure.
//!
//! # Status
//!
//! `MemBufferStore` is a pure proxy today. Establishing the wrapper
//! + factory machinery + cross-backend benches at this stage gives
//! us a baseline: any deviation from raw-backend numbers measures
//! the wrapper's per-call overhead. The real LRU + flusher logic
//! lands in a follow-up.

use super::types::{RecordKey, Store};
use crate::error::DbResult;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;

/// Configuration for `MemBufferRepo` / `MemBufferStore`.
///
/// All knobs have sensible defaults. They're surfaced so callers
/// can tune memory budget vs flush latency.
#[derive(Debug, Clone)]
pub struct MemBufferConfig {
    /// Max number of records held in the in-memory cache. Beyond
    /// this the LRU evicts cold entries (flushing dirty ones first).
    pub max_entries: usize,
    /// How often the background flusher wakes up (milliseconds).
    /// A high-rate write workload also signals the flusher on
    /// dirty-queue overflow; this is the idle interval.
    pub flush_interval_ms: u64,
    /// Max number of writes the flusher coalesces into one
    /// `set_many` / `remove_many` call against the inner store.
    pub flush_batch_size: usize,
}

impl Default for MemBufferConfig {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            flush_interval_ms: 100,
            flush_batch_size: 256,
        }
    }
}

// ============================================================================
// MemBufferStore — passthrough wrapper today; LRU + flusher tomorrow.
//
// Note: the `Repo` trait isn't object-safe (its `store_get` takes a
// generic `S: AsRef<str>`), so we cannot wrap `Arc<dyn Repo>` here.
// Repo-level integration lives in `shamir-engine`'s `BoxRepo` enum
// where a `MemBuffer` variant composes any other backend.
// ============================================================================

pub struct MemBufferStore {
    inner: Arc<dyn Store>,
    #[allow(dead_code)]
    config: MemBufferConfig,
    // Real impl will add:
    //   cache: Arc<Mutex<LruCache<RecordKey, CachedEntry>>>,
    //   dirty: Arc<Mutex<HashMap<RecordKey, DirtyOp>>>,
    //   flusher: Arc<FlusherState>,
    //   notify: Arc<Notify>,
}

impl MemBufferStore {
    pub fn new(inner: Arc<dyn Store>, config: MemBufferConfig) -> Self {
        Self { inner, config }
    }

    /// Access the underlying store. Used by tests and by the future
    /// flusher worker (it owns its own clone).
    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }
}

type RecordStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>> + Send>>;

#[async_trait]
impl Store for MemBufferStore {
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

    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }

    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        self.inner
            .iter_range_stream(start_inclusive, end_inclusive, batch_size)
    }

    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        self.inner
            .iter_range_stream_reverse(start_inclusive, end_inclusive, batch_size)
    }

    async fn flush(&self) -> DbResult<()> {
        // Real impl: drain the dirty queue first, then propagate.
        self.inner.flush().await
    }

    async fn insert_many(&self, values: Vec<Bytes>) -> DbResult<Vec<RecordKey>> {
        self.inner.insert_many(values).await
    }

    async fn set_many(
        &self,
        items: Vec<(RecordKey, Bytes)>,
    ) -> DbResult<Vec<bool>> {
        self.inner.set_many(items).await
    }

    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        self.inner.remove_many(keys).await
    }

    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        self.inner.get_many(keys).await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(deprecated)]

    use super::*;
    use crate::storage_in_memory::InMemoryRepo;
    use crate::types::{run_batch_store_tests, Repo};

    #[tokio::test]
    async fn membuffer_passthrough_passes_full_batch_suite() {
        // The cross-backend behaviour suite — covers single-key
        // CRUD, bulk insert/set/remove, range iteration forward
        // and reverse, get_many, flush, sequence ordering.
        use crate::types::Repo;
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("test").await.unwrap();
        let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(
            inner_store,
            MemBufferConfig::default(),
        ));
        run_batch_store_tests(store).await;
    }

    #[tokio::test]
    async fn membuffer_passthrough_inner_visible() {
        // For the passthrough version: every write made through the
        // MemBufferStore must be observable through the inner store
        // immediately (no buffering yet). After the real flusher
        // lands, this test will be replaced by an explicit
        // `store.flush().await; inner.get(...).is_ok()` assertion.
        let inner_repo = Arc::new(InMemoryRepo::new());
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = MemBufferStore::new(
            inner_store.clone(),
            MemBufferConfig::default(),
        );

        let value = Bytes::from_static(b"payload");
        let key = buffered.insert(value.clone()).await.unwrap();
        // Same value visible through the inner store directly —
        // proves passthrough semantics.
        let from_inner = inner_store.get(key).await.unwrap();
        assert_eq!(from_inner.as_ref(), value.as_ref());
    }
}
