use super::types::{RecordKey, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::common::{new_dash_map, TDashMap};
use async_trait::async_trait;
use async_stream::stream;
use bytes::Bytes;
use futures::stream::Stream;
use futures::StreamExt;
use std::pin::Pin;
use std::sync::Arc;

// ============================================================================
// CachedStore - in-memory cache wrapper over any Store
// ============================================================================

/// Cache layer that sits on top of another Store.
/// - Reads: Check local cache first, if missing - load from inner store
/// - Writes: Write to both cache and inner store (write-through)
/// - Keeps frequently accessed data in memory for fast access
pub struct CachedStore {
    inner: Arc<dyn Store>,
    cache: Arc<TDashMap<RecordKey, Bytes>>,
}

impl CachedStore {
    /// Create a new cached store wrapping the given inner store.
    pub fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            cache: Arc::new(new_dash_map()),
        }
    }

    /// Get reference to the inner store.
    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }

    /// Get reference to the cache (for inspection/debugging).
    pub fn cache(&self) -> &Arc<TDashMap<RecordKey, Bytes>> {
        &self.cache
    }

    /// Clear all cached entries (does NOT affect inner store).
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Get number of entries currently in cache.
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    /// Preload a key into cache (useful for warming up cache).
    pub async fn preload(&self, key: RecordKey) -> DbResult<()> {
        // Try to get from inner store, which will cache it
        let _ = self.get_with_cache(key).await?;
        Ok(())
    }

    /// Preload multiple keys into cache.
    pub async fn preload_many(&self, keys: Vec<RecordKey>) -> DbResult<usize> {
        let mut loaded = 0;
        for key in keys {
            if self.preload(key).await.is_ok() {
                loaded += 1;
            }
        }
        Ok(loaded)
    }

    /// Get value - check cache first, load from inner if missing.
    async fn get_with_cache(&self, key: RecordKey) -> DbResult<Bytes> {
        // Check cache first
        if let Some(ref_) = self.cache.get(&key) {
            return Ok(ref_.value().clone());
        }

        // Cache miss - load from inner store
        let value = self.inner.get(key.clone()).await?;

        // Store in cache for future access
        self.cache.insert(key, value.clone());

        Ok(value)
    }
}

#[async_trait]
impl Store for CachedStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        // Insert into inner store first
        let key = self.inner.insert(value.clone()).await?;

        // Cache the newly inserted value
        self.cache.insert(key.clone(), value);

        Ok(key)
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        // Write to both inner store and cache
        let created = self.inner.set(key.clone(), value.clone()).await?;

        // Update cache
        self.cache.insert(key, value);

        Ok(created)
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.get_with_cache(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        // Remove from both cache and inner store
        self.cache.remove(&key);
        self.inner.remove(key.clone()).await
    }

    async fn iter(&self) -> DbResult<Vec<(RecordKey, Bytes)>> {
        // Return all items from inner store
        // This ensures consistency but may be slower
        self.inner.iter().await
    }

    fn iter_stream(&self, batch_size: usize) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let inner = self.inner.clone();
        let cache = self.cache.clone();

        Box::pin(stream! {
            let mut stream = inner.iter_stream(batch_size);

            while let Some(batch_result) = stream.next().await {
                let batch: Vec<(RecordKey, Bytes)> = batch_result?;

                // Cache all items in this batch
                for (key, value) in &batch {
                    cache.insert(key.clone(), value.clone());
                }

                yield Ok(batch);
            }
        })
    }

    async fn scan_prefix(&self, prefix: Bytes) -> DbResult<Vec<(RecordKey, Bytes)>> {
        // Scan inner store and cache results
        let results = self.inner.scan_prefix(prefix.clone()).await?;

        // Cache all matching items
        for (key, value) in &results {
            self.cache.insert(key.clone(), value.clone());
        }

        Ok(results)
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let inner = self.inner.clone();
        let cache = self.cache.clone();

        Box::pin(stream! {
            let mut stream = inner.scan_prefix_stream(prefix, batch_size);

            while let Some(batch_result) = stream.next().await {
                let batch: Vec<(RecordKey, Bytes)> = batch_result?;

                // Cache all items in this batch
                for (key, value) in &batch {
                    cache.insert(key.clone(), value.clone());
                }

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

    #[tokio::test]
    async fn test_cached_store_basic() {
        // Create in-memory store as backing
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

        // Add some data to inner store
        let value1 = InnerValue::Str("from_inner".to_string());
        let key1 = inner.insert(value1.to_bytes()).await.unwrap();

        // Wrap with cached store
        let cached = CachedStore::new(inner.clone());

        // Cache should be empty initially
        assert_eq!(cached.cache_size(), 0);

        // First get should load from inner and cache it
        let retrieved = cached.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved).unwrap(), value1);
        assert_eq!(cached.cache_size(), 1);

        // Second get should hit cache
        let retrieved2 = cached.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved2).unwrap(), value1);
        assert_eq!(cached.cache_size(), 1); // Still 1, no new entry
    }

    #[tokio::test]
    async fn test_cached_store_insert() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new(inner.clone());

        // Insert through cached store
        let value1 = InnerValue::Str("cached_value".to_string());
        let key1 = cached.insert(value1.to_bytes()).await.unwrap();

        // Should be in cache
        assert_eq!(cached.cache_size(), 1);
        let from_cache = cached.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(from_cache).unwrap(), value1);

        // Should also be in inner store
        let from_inner = inner.get(key1).await.unwrap();
        assert_eq!(InnerValue::from_bytes(from_inner).unwrap(), value1);
    }

    #[tokio::test]
    async fn test_cached_store_set() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new(inner.clone());

        // Set new value through cached store
        let key = Bytes::from(b"test_key".to_vec());
        let value1 = InnerValue::Str("new_value".to_string());
        let created = cached.set(key.clone(), value1.to_bytes()).await.unwrap();
        assert!(created); // Should be new
        assert_eq!(cached.cache_size(), 1);

        // Update through cached store
        let value2 = InnerValue::Str("updated_value".to_string());
        let created2 = cached.set(key.clone(), value2.to_bytes()).await.unwrap();
        assert!(!created2); // Should be update

        // Both cache and inner should have updated value
        let from_cache = cached.get(key.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(from_cache).unwrap(), value2);

        let from_inner = inner.get(key).await.unwrap();
        assert_eq!(InnerValue::from_bytes(from_inner).unwrap(), value2);
    }

    #[tokio::test]
    async fn test_cached_store_remove() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new(inner.clone());

        // Insert a value
        let value1 = InnerValue::Str("to_delete".to_string());
        let key1 = cached.insert(value1.to_bytes()).await.unwrap();
        assert_eq!(cached.cache_size(), 1);

        // Remove through cached store
        let removed = cached.remove(key1.clone()).await.unwrap();
        assert!(removed);
        assert_eq!(cached.cache_size(), 0); // Cache cleared

        // Should not exist in cache or inner
        assert!(cached.get(key1.clone()).await.is_err());
        assert!(inner.get(key1).await.is_err());
    }

    #[tokio::test]
    async fn test_cached_store_clear_cache() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = CachedStore::new(inner.clone());

        // Insert some data
        let key1 = cached.insert(Bytes::from(&b"value1"[..])).await.unwrap();
        let key2 = cached.insert(Bytes::from(&b"value2"[..])).await.unwrap();
        assert_eq!(cached.cache_size(), 2);

        // Clear cache
        cached.clear_cache();
        assert_eq!(cached.cache_size(), 0);

        // Data should still be in inner store
        assert!(inner.get(key1.clone()).await.is_ok());
        assert!(inner.get(key2.clone()).await.is_ok());

        // Get should reload from inner
        let _ = cached.get(key1).await.unwrap();
        assert_eq!(cached.cache_size(), 1);
    }

    #[tokio::test]
    async fn test_cached_store_preload() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

        // Add data to inner
        let key1 = inner.insert(Bytes::from(&b"value1"[..])).await.unwrap();
        let key2 = inner.insert(Bytes::from(&b"value2"[..])).await.unwrap();

        let cached = CachedStore::new(inner.clone());
        assert_eq!(cached.cache_size(), 0);

        // Preload keys
        let loaded = cached.preload_many(vec![key1.clone(), key2.clone()]).await.unwrap();
        assert_eq!(loaded, 2);
        assert_eq!(cached.cache_size(), 2);
    }

    #[tokio::test]
    async fn test_cached_store_iter_stream_caches() {
        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

        // Add data to inner
        for i in 0..10 {
            let value = Bytes::from(format!("value_{}", i));
            inner.insert(value).await.unwrap();
        }

        let cached = CachedStore::new(inner.clone());
        assert_eq!(cached.cache_size(), 0);

        // Stream all items - should cache them
        let mut stream = cached.iter_stream(3);
        let mut count = 0;

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.unwrap();
            count += batch.len();
        }

        assert_eq!(count, 10);
        assert_eq!(cached.cache_size(), 10); // All items cached
    }

    #[tokio::test]
    async fn test_cached_store_scan_prefix_caches() {
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

        let cached = CachedStore::new(inner.clone());
        assert_eq!(cached.cache_size(), 0);

        // Scan prefix - should cache results
        let results = cached
            .scan_prefix(b"country:Russia:".to_vec().into())
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(cached.cache_size(), 3); // All results cached
    }

    #[tokio::test]
    async fn test_cached_store_concurrent_access() {
        use tokio::task::JoinSet;

        let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let cached = Arc::new(CachedStore::new(inner.clone()));
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

        // Spawn 50 concurrent reads
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

        // All writes should be in both cache and inner
        assert_eq!(cached.cache_size(), 50);
        assert_eq!(inner.iter().await.unwrap().len(), 50);
    }
}
