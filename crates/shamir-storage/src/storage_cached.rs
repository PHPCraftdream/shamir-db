use super::types::{RecordKey, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use shamir_types::types::common::{new_dash_map, TDashMap};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ============================================================================
// WriteMode - write strategy for CachedStore
// ============================================================================

/// Write strategy for cache operations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WriteMode {
    /// Write-through: wait for disk write before returning.
    /// Safer, slower. Use for critical data like indexes.
    Sync,

    /// Write-behind: write to cache immediately, disk write in background.
    /// Faster, but data may be lost on crash. Use for non-critical data.
    Async,
}

// ============================================================================
// CachedStore - in-memory full mirror of any Store
// ============================================================================

/// Full mirror cache that loads ALL data from inner store on creation.
///
/// ## Write Modes:
/// - `WriteMode::Sync`: write-through, waits for disk (safer for indexes)
/// - `WriteMode::Async`: write-behind, background writes (faster for data)
///
/// ## Behavior:
/// - Constructor: loads all data from inner into local cache
/// - Reads: from cache first, fallback to inner on miss (lazy load)
/// - Writes: depends on WriteMode (sync or async)
pub struct CachedStore {
    inner: Arc<dyn Store>,
    cache: Arc<TDashMap<RecordKey, Bytes>>,
    mode: WriteMode,
    pending_writes: Arc<AtomicUsize>,
}

impl CachedStore {
    async fn new_with_mode(inner: Arc<dyn Store>, mode: WriteMode) -> DbResult<Self> {
        use futures::StreamExt;

        let cache = Arc::new(new_dash_map());

        // Load ALL data from inner store into cache (streaming to avoid double allocation)
        let mut stream = inner.iter_stream(shamir_tunables::store_defaults::FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (key, value) in batch {
                cache.insert(key, value);
            }
        }

        Ok(Self {
            inner,
            cache,
            mode,
            pending_writes: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Create a new cached store with Sync write mode (safer, for indexes).
    /// Loads ALL data from inner store into cache.
    pub async fn new_sync(inner: Arc<dyn Store>) -> DbResult<Self> {
        Self::new_with_mode(inner, WriteMode::Sync).await
    }

    /// Create a new cached store with Async write mode (faster, for data).
    /// Loads ALL data from inner store into cache.
    pub async fn new_async(inner: Arc<dyn Store>) -> DbResult<Self> {
        Self::new_with_mode(inner, WriteMode::Async).await
    }

    /// Get reference to the inner store.
    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }

    /// Get reference to the cache (for inspection/debugging).
    pub fn cache(&self) -> &Arc<TDashMap<RecordKey, Bytes>> {
        &self.cache
    }

    /// Get write mode.
    pub fn mode(&self) -> WriteMode {
        self.mode
    }

    /// Get number of entries currently in cache.
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    /// Get number of pending async writes (0 for Sync mode).
    pub fn pending_writes(&self) -> usize {
        self.pending_writes.load(Ordering::Relaxed)
    }

    /// Reload all data from inner store (re-sync cache).
    /// Useful if inner store was modified externally.
    pub async fn reload(&self) -> DbResult<()> {
        use futures::StreamExt;

        // Clear current cache
        self.cache.clear();

        // Reload all data from inner (streaming)
        let mut stream = self
            .inner
            .iter_stream(shamir_tunables::store_defaults::FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (key, value) in batch {
                self.cache.insert(key, value);
            }
        }

        Ok(())
    }

    /// Flush all pending async writes (only for Async mode).
    /// For Sync mode, this is a no-op.
    pub async fn flush(&self) -> DbResult<()> {
        if matches!(self.mode, WriteMode::Sync) {
            return Ok(());
        }

        // Wait for pending writes to complete
        while self.pending_writes.load(Ordering::Relaxed) > 0 {
            tokio::task::yield_now().await;
        }

        Ok(())
    }
}

#[async_trait]
impl Store for CachedStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        // Insert ALWAYS needs to wait for inner to get the correct key
        // Async mode only applies to set/remove, not insert
        let key = self.inner.insert(value.clone()).await?;

        // Cache the value immediately
        self.cache.insert(key.clone(), value);
        Ok(key)
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        match self.mode {
            WriteMode::Sync => {
                // Write to both inner store and cache synchronously
                let created = self.inner.set(key.clone(), value.clone()).await?;
                self.cache.insert(key, value);
                Ok(created)
            }
            WriteMode::Async => {
                // Write to cache immediately
                let existed = self.cache.contains_key(&key);
                self.cache.insert(key.clone(), value.clone());

                // Background write to inner store
                let inner = self.inner.clone();
                let pending = self.pending_writes.clone();

                pending.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    // §B8: WriteMode::Async is fire-and-forget by design,
                    // but a swallowed `Err` silently loses durability.
                    // Log so an operator gets a signal under sustained
                    // backing-store failure; the cache already holds the
                    // value so subsequent reads still succeed.
                    if let Err(e) = inner.set(key, value).await {
                        log::error!("storage_cached async write to backing store failed: {}", e);
                    }
                    pending.fetch_sub(1, Ordering::Relaxed);
                });

                Ok(!existed)
            }
        }
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        // Try cache first
        if let Some(ref_) = self.cache.get(&key) {
            return Ok(ref_.value().clone());
        }

        // Cache miss - load from inner store and cache it
        // This handles cases where inner was modified externally
        let value = self.inner.get(key.clone()).await?;

        // Store in cache for future access
        self.cache.insert(key, value.clone());

        Ok(value)
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let existed = self.cache.remove(&key).is_some();

        match self.mode {
            WriteMode::Sync => self.inner.remove(key).await,
            WriteMode::Async => {
                // Background delete
                let inner = self.inner.clone();
                let pending = self.pending_writes.clone();

                pending.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    // §B8: WriteMode::Async is fire-and-forget by design,
                    // but a swallowed `Err` silently loses durability.
                    // Log so an operator gets a signal under sustained
                    // backing-store failure.
                    if let Err(e) = inner.remove(key).await {
                        log::error!(
                            "storage_cached async remove from backing store failed: {}",
                            e
                        );
                    }
                    pending.fetch_sub(1, Ordering::Relaxed);
                });

                Ok(existed)
            }
        }
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let cache = self.cache.clone();

        Box::pin(stream! {
            let all_items: Vec<(RecordKey, Bytes)> = cache
                .iter()
                .map(|ref_| (ref_.key().clone(), ref_.value().clone()))
                .collect();

            let mut items = all_items;
            items.sort_by(|a, b| a.0.cmp(&b.0)); // Sort for consistent ordering

            while !items.is_empty() {
                let batch: Vec<_> = items
                    .drain(..std::cmp::min(batch_size, items.len()))
                    .collect();

                yield Ok(batch);
            }
        })
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let cache = self.cache.clone();
        let prefix_slice = prefix.to_vec();

        Box::pin(stream! {
            let matching_items: Vec<(RecordKey, Bytes)> = cache
                .iter()
                .filter(|ref_| ref_.key().starts_with(&prefix_slice[..]))
                .map(|ref_| (ref_.key().clone(), ref_.value().clone()))
                .collect();

            let mut items = matching_items;
            items.sort_by(|a, b| a.0.cmp(&b.0)); // Sort for consistent ordering

            while !items.is_empty() {
                let batch: Vec<_> = items
                    .drain(..std::cmp::min(batch_size, items.len()))
                    .collect();

                yield Ok(batch);
            }
        })
    }

    /// Delegate to inner store's `transact`, then invalidate cache
    /// entries for all touched keys. The cache layer itself doesn't
    /// add atomicity — that comes from the inner backend.
    async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
        // Collect keys before delegating (ops is moved into inner).
        let keys: Vec<RecordKey> = ops
            .iter()
            .map(|op| match op {
                super::types::KvOp::Set(k, _) | super::types::KvOp::Remove(k) => k.clone(),
            })
            .collect();

        self.inner.transact(ops).await?;

        // Invalidate cache for affected keys so subsequent reads
        // see the transacted state, not stale cached values.
        for k in keys {
            self.cache.remove(&k);
        }
        Ok(())
    }

    /// Pass-through for buffer config: a CachedStore doesn't have
    /// its own buffer knobs but the underlying store likely does
    /// (especially when stacked Cached → MemBuffer → raw).
    async fn apply_buffer_config(
        &self,
        config: &crate::storage_membuffer::MemBufferConfig,
    ) -> DbResult<()> {
        self.inner.apply_buffer_config(config).await
    }

    async fn raw_backend(&self) -> Option<Arc<dyn Store>> {
        Some(Arc::clone(&self.inner))
    }

    /// Drain pending async writes and propagate the flush down to
    /// the inner store. Reachable through `Arc<dyn Store>` —
    /// without this override the trait dispatcher would land on
    /// the default no-op and async-mode writes would not become
    /// durable on a `flush()` callsite.
    async fn flush(&self) -> DbResult<()> {
        // Wait for the in-flight background `set`/`remove` tasks
        // (only present in `WriteMode::Async`). For `Sync` mode
        // pending_writes is always 0 and the loop body never runs.
        while self.pending_writes.load(Ordering::Relaxed) > 0 {
            tokio::task::yield_now().await;
        }
        // Now ensure the inner store's own buffered state lands.
        self.inner.flush().await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(deprecated)]

    use super::*;
    use crate::storage_in_memory::InMemoryStore;
    use crate::types::collect_stream;
    use futures::StreamExt;
    use shamir_types::types::value::InnerValue;
    use tokio::time::{sleep, Duration};

    /// Regression: `flush` must work through `Arc<dyn Store>`. The
    /// inherent `CachedStore::flush` was reachable on a concrete
    /// `CachedStore`, but the trait dispatch went to the default
    /// no-op — async-mode pending writes were never drained when
    /// callers held `Arc<dyn Store>` (which is how every engine
    /// path holds it).
    #[tokio::test]
    async fn test_cached_store_flush_via_dyn_store() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached_concrete = Arc::new(CachedStore::new_async(inner.clone()).await.unwrap());
        let cached_dyn: Arc<dyn Store> = cached_concrete.clone();

        // Pump enough async writes that at least one is still pending
        // when the spawn_blocking returns. With a tiny in-memory inner
        // store the spawned set tasks complete almost instantly, so
        // we issue many in a tight loop to widen the window.
        let mut keys = Vec::new();
        for _ in 0..200 {
            let k = RecordKey::copy_from_slice(
                shamir_types::types::record_id::RecordId::new().as_bytes(),
            );
            cached_dyn
                .set(k.clone(), Bytes::from_static(b"async-write"))
                .await
                .unwrap();
            keys.push(k);
        }

        // Call flush through the trait. AFTER this returns,
        // pending_writes must be 0.
        cached_dyn.flush().await.unwrap();
        assert_eq!(
            cached_concrete.pending_writes(),
            0,
            "Store::flush via dyn dispatch must drain CachedStore pending writes"
        );

        // Every value must be visible through the inner store —
        // proof that the background sets landed.
        for k in &keys {
            let got = inner.get(k.clone()).await.expect("inner has the value");
            assert_eq!(got.as_ref(), b"async-write");
        }
    }

    #[tokio::test]
    async fn test_cached_store_sync_mode() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

        // Insert should be in both cache and inner immediately
        let value1 = InnerValue::Str("sync_value".to_string());
        let key1 = cached.insert(value1.to_bytes().unwrap()).await.unwrap();

        assert!(cached.cache.get(&key1).is_some());
        assert!(inner.get(key1.clone()).await.is_ok());
    }

    #[tokio::test]
    async fn test_cached_store_async_mode() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_async(inner.clone()).await.unwrap();

        // Insert - immediately in cache
        let value1 = InnerValue::Str("async_value".to_string());
        let key1 = cached.insert(value1.to_bytes().unwrap()).await.unwrap();

        assert!(cached.cache.get(&key1).is_some());

        // May not be in inner yet (background write)
        // But eventually should be
        sleep(Duration::from_millis(10)).await;
        assert!(inner.get(key1).await.is_ok());
    }

    #[tokio::test]
    async fn test_cached_store_async_pending_writes() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_async(inner.clone()).await.unwrap();

        // Start multiple async writes
        for i in 0..10 {
            let key = format!("key_{}", i);
            let value = Bytes::from(key.clone());
            cached.set(key.into(), value).await.unwrap();
        }

        // Should have some pending writes
        let pending = cached.pending_writes();
        assert!(pending > 0); // Might have completed already

        // Flush and wait for completion
        cached.flush().await.unwrap();
        assert_eq!(cached.pending_writes(), 0);

        // All data should be in inner now
        assert_eq!(
            collect_stream(inner.iter_stream(1000)).await.unwrap().len(),
            10
        );
    }

    #[tokio::test]
    async fn test_cached_store_sync_no_pending() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

        cached
            .set(Bytes::from("key"), Bytes::from("value"))
            .await
            .unwrap();

        // Sync mode - no pending writes
        assert_eq!(cached.pending_writes(), 0);
        cached.flush().await.unwrap(); // Should be no-op
    }

    #[tokio::test]
    async fn test_cached_store_loads_all_on_creation() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

        // Add some data to inner store BEFORE creating cached store
        for i in 0..10 {
            let value = InnerValue::Int(i);
            inner.insert(value.to_bytes().unwrap()).await.unwrap();
        }

        // Create cached store - should load all data
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

        // All data should be in cache
        assert_eq!(cached.cache_size(), 10);

        // Can retrieve all items without touching inner store
        let all_from_cache = collect_stream(cached.iter_stream(1000)).await.unwrap();
        assert_eq!(all_from_cache.len(), 10);
    }

    #[tokio::test]
    async fn test_cached_get_with_fallback() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

        // Add data to inner
        let value1 = InnerValue::Str("test_value".to_string());
        let key1 = inner.insert(value1.to_bytes().unwrap()).await.unwrap();

        // Create cached store - loads all data
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

        // Get should work (from cache)
        let retrieved = cached.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved).unwrap(), value1);

        // Remove from inner store
        inner.remove(key1.clone()).await.unwrap();

        // Get should STILL work (from cache, inner is empty now)
        let retrieved2 = cached.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved2).unwrap(), value1);

        // Add NEW data to inner directly (external modification)
        let value2 = InnerValue::Str("new_external_value".to_string());
        let key2 = inner.insert(value2.to_bytes().unwrap()).await.unwrap();

        // Get should work with fallback - loads from inner and caches it
        let retrieved3 = cached.get(key2.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved3).unwrap(), value2);
        assert_eq!(cached.cache_size(), 2); // Now both keys in cache
    }

    #[tokio::test]
    async fn test_cached_insert_mirrors_to_inner() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

        assert_eq!(cached.cache_size(), 0);

        // Insert through cached store
        let value1 = InnerValue::Str("mirrored".to_string());
        let key1 = cached.insert(value1.to_bytes().unwrap()).await.unwrap();

        // Should be in cache
        assert_eq!(cached.cache_size(), 1);
        let from_cache = cached.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(from_cache).unwrap(), value1);

        // Should ALSO be in inner store
        let from_inner = inner.get(key1).await.unwrap();
        assert_eq!(InnerValue::from_bytes(from_inner).unwrap(), value1);
    }

    #[tokio::test]
    async fn test_cached_set_mirrors_to_inner() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

        // Set new value
        let key = Bytes::from(b"test_key".to_vec());
        let value1 = InnerValue::Str("new_value".to_string());
        let created = cached
            .set(key.clone(), value1.to_bytes().unwrap())
            .await
            .unwrap();
        assert!(created);

        // Both cache and inner should have it
        let from_cache = cached.get(key.clone()).await.unwrap();
        let from_inner = inner.get(key.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(from_cache).unwrap(), value1);
        assert_eq!(InnerValue::from_bytes(from_inner).unwrap(), value1);

        // Update value
        let value2 = InnerValue::Str("updated".to_string());
        let created2 = cached
            .set(key.clone(), value2.to_bytes().unwrap())
            .await
            .unwrap();
        assert!(!created2);

        // Both should reflect update
        let from_cache = cached.get(key.clone()).await.unwrap();
        let from_inner = inner.get(key).await.unwrap();
        assert_eq!(InnerValue::from_bytes(from_cache).unwrap(), value2);
        assert_eq!(InnerValue::from_bytes(from_inner).unwrap(), value2);
    }

    #[tokio::test]
    async fn test_cached_remove_mirrors_to_inner() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

        // Insert data
        let key1 = cached.insert(Bytes::from(&b"value1"[..])).await.unwrap();
        assert_eq!(cached.cache_size(), 1);

        // Remove through cached store
        let removed = cached.remove(key1.clone()).await.unwrap();
        assert!(removed);
        assert_eq!(cached.cache_size(), 0);

        // Should be removed from both
        assert!(cached.get(key1.clone()).await.is_err());
        assert!(inner.get(key1).await.is_err());
    }

    #[tokio::test]
    async fn test_cached_reload_resyncs_with_inner() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

        // Initial data
        let key1 = inner.insert(Bytes::from(&b"value1"[..])).await.unwrap();

        // Create cached store
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();
        assert_eq!(cached.cache_size(), 1);

        // Add more data to inner directly
        let key2 = inner.insert(Bytes::from(&b"value2"[..])).await.unwrap();
        let key3 = inner.insert(Bytes::from(&b"value3"[..])).await.unwrap();

        // Cache still has only 1 item
        assert_eq!(cached.cache_size(), 1);

        // Reload - should fetch all data from inner
        cached.reload().await.unwrap();
        assert_eq!(cached.cache_size(), 3);

        // Now can get all items from cache
        assert!(cached.get(key1).await.is_ok());
        assert!(cached.get(key2).await.is_ok());
        assert!(cached.get(key3).await.is_ok());
    }

    #[tokio::test]
    async fn test_cached_iter_stream_from_cache() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

        // Add data to inner
        for i in 0..20 {
            let value = Bytes::from(format!("value_{}", i));
            inner.insert(value).await.unwrap();
        }

        // Create cached store
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();
        assert_eq!(cached.cache_size(), 20);

        // Stream from cache (no inner access)
        let mut stream = cached.iter_stream(5);
        let mut count = 0;

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.unwrap();
            count += batch.len();
        }

        assert_eq!(count, 20);
    }

    #[tokio::test]
    async fn test_cached_concurrent_access() {
        use tokio::task::JoinSet;

        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = Arc::new(CachedStore::new_sync(inner.clone()).await.unwrap());
        let mut join_set = JoinSet::new();

        // Spawn 50 concurrent writes
        for i in 0..50 {
            let store = cached.clone();
            join_set.spawn(async move {
                let key = format!("key_{}", i);
                let value = Bytes::from(key.clone());
                store.set(key.into(), value).await.unwrap();
            });
        }

        // Spawn 50 concurrent reads (all from cache, no inner access)
        for i in 0..50 {
            let store = cached.clone();
            join_set.spawn(async move {
                let key = format!("key_{}", i);
                let _ = store.get(key.into()).await;
            });
        }

        // All tasks should complete
        while let Some(result) = join_set.join_next().await {
            result.unwrap();
        }

        // All writes mirrored to both cache and inner
        assert_eq!(cached.cache_size(), 50);
        assert_eq!(
            collect_stream(inner.iter_stream(1000)).await.unwrap().len(),
            50
        );
    }

    #[tokio::test]
    async fn raw_backend_unwraps_cached() {
        let seed_key = Bytes::from_static(b"cached-seed-key");
        let seed_val = Bytes::from_static(b"cached-seed-val");

        let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        inner.set(seed_key.clone(), seed_val.clone()).await.unwrap();

        let cached: Arc<dyn Store> =
            Arc::new(CachedStore::new_sync(Arc::clone(&inner)).await.unwrap());

        let raw = cached
            .raw_backend()
            .await
            .expect("CachedStore returns Some");
        // raw is the same inner — observable via the seeded value
        assert_eq!(raw.get(seed_key).await.unwrap(), seed_val);
    }

    // Renamed from `test_cached_async_mode_crash_simulation` — no
    // crash is simulated. The test verifies that the async-mode cache
    // may lag the inner store before `flush()` and that all writes
    // are durable in the inner store after `flush()` completes.
    #[tokio::test]
    async fn test_cached_async_mode_persists_after_flush() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_async(inner.clone()).await.unwrap();

        // Write data.
        for i in 0..5 {
            let key = format!("key_{}", i);
            let value = Bytes::from(key.clone());
            cached.set(key.into(), value).await.unwrap();
        }

        // Data is in cache.
        assert_eq!(cached.cache_size(), 5);

        // Inner store may lag (async writes).
        let inner_count = collect_stream(inner.iter_stream(1000)).await.unwrap().len();
        assert!(inner_count <= 5);

        // After flush all writes are durable in the inner store.
        cached.flush().await.unwrap();
        assert_eq!(
            collect_stream(inner.iter_stream(1000)).await.unwrap().len(),
            5
        );
    }
}
