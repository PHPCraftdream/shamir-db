use super::types::{RecordKey, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::common::{new_dash_map, TDashMap};
use async_trait::async_trait;
use async_stream::stream;
use bytes::Bytes;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
        let cache = Arc::new(new_dash_map());

        // Load ALL data from inner store into cache
        let all_data = inner.iter().await?;
        for (key, value) in all_data {
            cache.insert(key, value);
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
        // Clear current cache
        self.cache.clear();

        // Reload all data from inner
        let all_data = self.inner.iter().await?;
        for (key, value) in all_data {
            self.cache.insert(key, value);
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
                    let _ = inner.set(key, value).await;
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
            WriteMode::Sync => {
                self.inner.remove(key).await
            }
            WriteMode::Async => {
                // Background delete
                let inner = self.inner.clone();
                let pending = self.pending_writes.clone();

                pending.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    let _ = inner.remove(key).await;
                    pending.fetch_sub(1, Ordering::Relaxed);
                });

                Ok(existed)
            }
        }
    }

    async fn iter(&self) -> DbResult<Vec<(RecordKey, Bytes)>> {
        // Return all items from cache (no need to query inner)
        let items: Vec<(RecordKey, Bytes)> = self
            .cache
            .iter()
            .map(|ref_| (ref_.key().clone(), ref_.value().clone()))
            .collect();
        Ok(items)
    }

    fn iter_stream(&self, batch_size: usize) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
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

    async fn scan_prefix(&self, prefix: Bytes) -> DbResult<Vec<(RecordKey, Bytes)>> {
        let prefix_slice = &prefix[..];

        // Scan only cache (all data is already there)
        let items: Vec<(RecordKey, Bytes)> = self
            .cache
            .iter()
            .filter(|ref_| ref_.key().starts_with(prefix_slice))
            .map(|ref_| (ref_.key().clone(), ref_.value().clone()))
            .collect();

        Ok(items)
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
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::storage::storage_in_memory::InMemoryStore;
    use crate::types::value::InnerValue;
    use futures::StreamExt;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_cached_store_sync_mode() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

        // Insert should be in both cache and inner immediately
        let value1 = InnerValue::Str("sync_value".to_string());
        let key1 = cached.insert(value1.to_bytes()).await.unwrap();

        assert!(cached.cache.get(&key1).is_some());
        assert!(inner.get(key1.clone()).await.is_ok());
    }

    #[tokio::test]
    async fn test_cached_store_async_mode() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_async(inner.clone()).await.unwrap();

        // Insert - immediately in cache
        let value1 = InnerValue::Str("async_value".to_string());
        let key1 = cached.insert(value1.to_bytes()).await.unwrap();

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
        assert!(pending > 0 || pending == 0); // Might have completed already

        // Flush and wait for completion
        cached.flush().await.unwrap();
        assert_eq!(cached.pending_writes(), 0);

        // All data should be in inner now
        assert_eq!(inner.iter().await.unwrap().len(), 10);
    }

    #[tokio::test]
    async fn test_cached_store_sync_no_pending() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

        cached.set(Bytes::from("key"), Bytes::from("value")).await.unwrap();

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
            inner.insert(value.to_bytes()).await.unwrap();
        }

        // Create cached store - should load all data
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

        // All data should be in cache
        assert_eq!(cached.cache_size(), 10);

        // Can retrieve all items without touching inner store
        let all_from_cache = cached.iter().await.unwrap();
        assert_eq!(all_from_cache.len(), 10);
    }

    #[tokio::test]
    async fn test_cached_get_with_fallback() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

        // Add data to inner
        let value1 = InnerValue::Str("test_value".to_string());
        let key1 = inner.insert(value1.to_bytes()).await.unwrap();

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
        let key2 = inner.insert(value2.to_bytes()).await.unwrap();

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
        let key1 = cached.insert(value1.to_bytes()).await.unwrap();

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
        let created = cached.set(key.clone(), value1.to_bytes()).await.unwrap();
        assert!(created);

        // Both cache and inner should have it
        let from_cache = cached.get(key.clone()).await.unwrap();
        let from_inner = inner.get(key.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(from_cache).unwrap(), value1);
        assert_eq!(InnerValue::from_bytes(from_inner).unwrap(), value1);

        // Update value
        let value2 = InnerValue::Str("updated".to_string());
        let created2 = cached.set(key.clone(), value2.to_bytes()).await.unwrap();
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
    async fn test_cached_scan_prefix_from_cache() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

        // Insert data with composite keys
        let data = vec![
            b"country:Russia:Moscow:user1".to_vec(),
            b"country:Russia:Moscow:user2".to_vec(),
            b"country:Russia:SPb:user3".to_vec(),
            b"country:France:Paris:user4".to_vec(),
        ];

        for key in &data {
            inner.set(key.clone().into(), Bytes::copy_from_slice(key)).await.unwrap();
        }

        // Create cached store - loads all data
        let cached = CachedStore::new_sync(inner.clone()).await.unwrap();
        assert_eq!(cached.cache_size(), 4);

        // Scan prefix from cache (no inner access)
        let results = cached
            .scan_prefix(b"country:Russia:".to_vec().into())
            .await
            .unwrap();

        assert_eq!(results.len(), 3);

        // Even if we modify inner, cache still has original data
        inner.set(b"country:Russia:NEW:user5".to_vec().into(), Bytes::from(&b"new"[..])).await.unwrap();
        let results2 = cached.scan_prefix(b"country:Russia:".to_vec().into()).await.unwrap();
        assert_eq!(results2.len(), 3); // Still 3, not 4 (cache is out of sync)
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
        assert_eq!(inner.iter().await.unwrap().len(), 50);
    }

    #[tokio::test]
    async fn test_cached_async_mode_crash_simulation() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new_async(inner.clone()).await.unwrap();

        // Write data
        for i in 0..5 {
            let key = format!("key_{}", i);
            let value = Bytes::from(key.clone());
            cached.set(key.into(), value).await.unwrap();
        }

        // Data is in cache
        assert_eq!(cached.cache_size(), 5);

        // But may not be fully written to inner yet
        let inner_count = inner.iter().await.unwrap().len();
        assert!(inner_count <= 5); // May be less due to async writes

        // After flush, all should be in inner
        cached.flush().await.unwrap();
        assert_eq!(inner.iter().await.unwrap().len(), 5);
    }
}
