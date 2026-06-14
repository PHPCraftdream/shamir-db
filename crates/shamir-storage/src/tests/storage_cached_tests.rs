#![allow(deprecated)]

use crate::storage_cached::CachedStore;
use crate::storage_in_memory::InMemoryStore;
use crate::tests::types_tests::collect_stream;
use crate::types::{RecordKey, Store};
use bytes::Bytes;
use futures::StreamExt;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;
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
        let k = RecordKey::copy_from_slice(RecordId::new().as_bytes());
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

    assert!(cached.cache().peek_with(&key1, |_, _| ()).is_some());
    assert!(inner.get(key1.clone()).await.is_ok());
}

#[tokio::test]
async fn test_cached_store_async_mode() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_async(inner.clone()).await.unwrap();

    // Insert - immediately in cache
    let value1 = InnerValue::Str("async_value".to_string());
    let key1 = cached.insert(value1.to_bytes().unwrap()).await.unwrap();

    assert!(cached.cache().peek_with(&key1, |_, _| ()).is_some());

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

    let cached: Arc<dyn Store> = Arc::new(CachedStore::new_sync(Arc::clone(&inner)).await.unwrap());

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
