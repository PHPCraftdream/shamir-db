#![allow(deprecated)]

use crate::storage_cached::CachedStore;
use crate::storage_in_memory::InMemoryStore;
use crate::tests::types_tests::collect_stream;
use crate::types::{KvOp, RecordKey, Store};
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
        let k = RecordKey::from_slice(RecordId::new().as_bytes());
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
        cached
            .set(RecordKey::from(key.into_bytes()), value)
            .await
            .unwrap();
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
        .set(Bytes::from("key").into(), Bytes::from("value"))
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
    let key = RecordKey::from(b"test_key".to_vec());
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
            store
                .set(RecordKey::from(key.into_bytes()), value)
                .await
                .unwrap();
        });
    }

    // Spawn 50 concurrent reads (all from cache, no inner access)
    for i in 0..50 {
        let store = cached.clone();
        join_set.spawn(async move {
            let key = format!("key_{}", i);
            let _ = store.get(RecordKey::from(key.into_bytes())).await;
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
    let seed_key = RecordKey::from_slice(b"cached-seed-key");
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
        cached
            .set(RecordKey::from(key.into_bytes()), value)
            .await
            .unwrap();
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

// ============================================================================
// transact: populate-on-Set (audit 2026-07-06-perf-radical-o-notation §1.4)
// ============================================================================

/// `transact` with a `Set` op must POPULATE the cache with the fresh
/// value (not invalidate it), so a subsequent read-after-write hits the
/// cache (RAM) instead of the (disk) backend. The previous code removed
/// every touched key post-commit, systematically keeping the cache cold
/// on just-written data — a ×10-100 read-after-write cost multiplier.
///
/// We assert via the cache's own internal state: immediately after
/// `transact` commits, the just-written key must be present in the
/// cache (proving the populate path ran, not the invalidate path).
#[tokio::test]
async fn test_transact_set_populates_cache() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();
    assert_eq!(cached.cache_size(), 0);

    let key = RecordKey::from_slice(b"raw-key");
    let value = Bytes::from_static(b"fresh-value");

    cached
        .transact(vec![KvOp::Set(key.clone(), value.clone())])
        .await
        .unwrap();

    // The cache MUST now hold the just-written value (populate, not
    // invalidate). `cache_size` is 1 and a direct cache lookup returns
    // the fresh bytes — proof the entry is in RAM, not evicted.
    assert_eq!(cached.cache_size(), 1);
    let from_cache = cached
        .cache()
        .peek_with(&key, |_, v| v.clone())
        .expect("Set op must populate the cache, not invalidate it");
    assert_eq!(from_cache, value);

    // And `get` (which checks cache first) returns the fresh value
    // without needing a backend round-trip.
    let got = cached.get(key.clone()).await.unwrap();
    assert_eq!(got, value);
}

/// `transact` with a `Set` op on a key that ALREADY exists in the
/// cache must UPDATE the cached value in place (size unchanged) — the
/// populate path uses the same upsert discipline as the single-key
/// `set`, so a stale cached value is replaced with the fresh one
/// rather than the entry being removed.
#[tokio::test]
async fn test_transact_set_updates_existing_cached_entry() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

    let key = RecordKey::from_slice(b"upsert-key");
    let old = Bytes::from_static(b"old-value");
    let fresh = Bytes::from_static(b"fresh-value");

    // Seed the cache with the old value.
    cached.set(key.clone(), old.clone()).await.unwrap();
    assert_eq!(cached.cache_size(), 1);

    // transact a Set on the SAME key → must replace, not evict.
    cached
        .transact(vec![KvOp::Set(key.clone(), fresh.clone())])
        .await
        .unwrap();

    // Size unchanged (upsert, not remove-then-nothing).
    assert_eq!(cached.cache_size(), 1);
    let from_cache = cached
        .cache()
        .peek_with(&key, |_, v| v.clone())
        .expect("key still cached after transact Set");
    assert_eq!(
        from_cache, fresh,
        "cached value must reflect the committed update, not stay stale"
    );
}

/// `transact` with a `Remove` op must STILL evict the cache entry —
/// this is the correct behaviour for deletes and must NOT change.
/// Regression guard for the populate-on-Set fix (the Remove path
/// stays an invalidate).
#[tokio::test]
async fn test_transact_remove_evicts_cache() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

    let key = RecordKey::from_slice(b"del-key");
    cached
        .set(key.clone(), Bytes::from_static(b"v"))
        .await
        .unwrap();
    assert_eq!(cached.cache_size(), 1);

    cached
        .transact(vec![KvOp::Remove(key.clone())])
        .await
        .unwrap();

    // Remove MUST evict — cache is now empty, and the key is gone.
    assert_eq!(cached.cache_size(), 0);
    assert!(
        cached.cache().peek_with(&key, |_, _| ()).is_none(),
        "Remove op must evict the cache entry"
    );
    assert!(cached.get(key).await.is_err());
}

/// Mixed `transact` batch (Set + Remove): each op applies its own
/// cache action. The Set populates; the Remove evicts. Confirms the
/// `CacheAction` enum correctly distinguishes the two within a single
/// batch.
#[tokio::test]
async fn test_transact_mixed_set_remove() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

    let keep_key = RecordKey::from_slice(b"keep-key");
    let del_key = RecordKey::from_slice(b"del-key");

    // Seed both.
    cached
        .set(keep_key.clone(), Bytes::from_static(b"k-old"))
        .await
        .unwrap();
    cached
        .set(del_key.clone(), Bytes::from_static(b"d-old"))
        .await
        .unwrap();
    assert_eq!(cached.cache_size(), 2);

    cached
        .transact(vec![
            KvOp::Set(keep_key.clone(), Bytes::from_static(b"k-fresh")),
            KvOp::Remove(del_key.clone()),
        ])
        .await
        .unwrap();

    // keep_key: populated with the fresh value (size stable at 1 — one
    // removed, one updated-in-place).
    assert_eq!(cached.cache_size(), 1);
    let keep_val = cached
        .cache()
        .peek_with(&keep_key, |_, v| v.clone())
        .expect("keep_key populated by Set");
    assert_eq!(keep_val, Bytes::from_static(b"k-fresh"));

    // del_key: evicted by Remove.
    assert!(
        cached.cache().peek_with(&del_key, |_, _| ()).is_none(),
        "del_key evicted by Remove"
    );
}

/// A multi-key Set batch populates ALL keys into the cache (no cap on
/// how much of a batch gets cached — matching the single-key `set`
/// path's "always cache on write" behaviour). Size bumps by the number
/// of genuinely-new keys.
#[tokio::test]
async fn test_transact_set_many_populates_all() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

    let ops: Vec<KvOp> = (0..50)
        .map(|i| {
            KvOp::Set(
                Bytes::from(format!("batch-key-{:#04}", i)).into(),
                Bytes::from(format!("val-{}", i)),
            )
        })
        .collect();

    cached.transact(ops).await.unwrap();

    // All 50 keys are now in the cache (populate, not invalidate).
    assert_eq!(cached.cache_size(), 50);
    for i in 0..50 {
        let k: RecordKey = Bytes::from(format!("batch-key-{:#04}", i)).into();
        let v = cached
            .cache()
            .peek_with(&k, |_, v| v.clone())
            .unwrap_or_else(|| panic!("key {} must be cached after transact Set", i));
        assert_eq!(v, Bytes::from(format!("val-{}", i)));
    }
}

// ============================================================================
// iter_stream / scan_prefix_stream: incremental correctness preserved
// (audit 2026-07-06-perf-radical-o-notation §1.3)
// ============================================================================

/// `iter_stream` after the incrementalization must still yield the
/// FULL result set, in ascending TreeIndex key order, across multiple
/// batches. Correctness guard for the cursor/resume rework.
#[tokio::test]
async fn test_iter_stream_full_results_sorted() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

    // Seed 23 keys with sortable byte keys (pad so lex order is stable).
    for i in 0..23u32 {
        let key: RecordKey = Bytes::from(format!("key-{:05}", i)).into();
        cached
            .set(key, Bytes::from(i.to_be_bytes().to_vec()))
            .await
            .unwrap();
    }
    assert_eq!(cached.cache_size(), 23);

    // batch_size = 5 → expect 5 batches (5,5,5,5,3).
    let collected = collect_stream(cached.iter_stream(5)).await.unwrap();

    assert_eq!(collected.len(), 23, "all 23 entries must be yielded");

    // Verify ascending key order across the whole stream.
    for w in collected.windows(2) {
        assert!(
            w[0].0 < w[1].0,
            "iter_stream must yield ascending key order"
        );
    }

    // Verify the first and last keys are the expected min/max.
    assert_eq!(collected.first().unwrap().0, Bytes::from("key-00000"));
    assert_eq!(collected.last().unwrap().0, Bytes::from("key-00022"));
}

/// `iter_stream` with batch_size larger than the corpus yields a
/// single (short) batch — the cursor must terminate correctly when the
/// first batch is short (no off-by-one infinite loop).
#[tokio::test]
async fn test_iter_stream_batch_larger_than_corpus() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

    for i in 0..3u32 {
        cached
            .set(
                Bytes::from(format!("k{}", i)).into(),
                Bytes::from_static(b"v"),
            )
            .await
            .unwrap();
    }

    // batch_size 1000 >> 3 entries → one batch of 3, then terminate.
    let collected = collect_stream(cached.iter_stream(1000)).await.unwrap();
    assert_eq!(collected.len(), 3);
}

/// `iter_stream` on an EMPTY cache yields nothing (no empty batches,
/// no panic).
#[tokio::test]
async fn test_iter_stream_empty_cache() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();
    assert_eq!(cached.cache_size(), 0);

    let collected = collect_stream(cached.iter_stream(10)).await.unwrap();
    assert!(collected.is_empty());
}

/// `iter_stream` with batch_size == corpus size (exact multiple)
/// terminates correctly — the last full batch is followed by an empty
/// re-query that ends the stream. Guards against the
/// "multiple-of-batch-size" off-by-one that a naive cursor can hit.
#[tokio::test]
async fn test_iter_stream_exact_batch_multiple() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

    for i in 0..10u32 {
        cached
            .set(
                Bytes::from(format!("k{:02}", i)).into(),
                Bytes::from_static(b"v"),
            )
            .await
            .unwrap();
    }

    // 10 entries, batch_size 5 → two full batches, then an empty re-query.
    let collected = collect_stream(cached.iter_stream(5)).await.unwrap();
    assert_eq!(collected.len(), 10);

    // batch_size 10 → exactly one full batch.
    let collected = collect_stream(cached.iter_stream(10)).await.unwrap();
    assert_eq!(collected.len(), 10);

    // batch_size 2 → five full batches.
    let collected = collect_stream(cached.iter_stream(2)).await.unwrap();
    assert_eq!(collected.len(), 10);
}

/// `scan_prefix_stream` after the incrementalization must still yield
/// the FULL prefix-matching subset, in ascending key order, and STOP
/// at the prefix boundary (no leakage of non-matching keys).
#[tokio::test]
async fn test_scan_prefix_stream_full_results_sorted_and_bounded() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

    // Seed keys under two prefixes plus some non-matching keys that
    // sort AFTER the prefix range (to catch boundary leakage).
    for i in 0..7u32 {
        cached
            .set(
                Bytes::from(format!("pfxA-{:03}", i)).into(),
                Bytes::from_static(b"v"),
            )
            .await
            .unwrap();
    }
    for i in 0..4u32 {
        cached
            .set(
                Bytes::from(format!("pfxB-{:03}", i)).into(),
                Bytes::from_static(b"v"),
            )
            .await
            .unwrap();
    }
    // `pfxC` sorts after `pfxA` — must NOT leak into the pfxA scan.
    cached
        .set(
            Bytes::from_static(b"pfxC-000").into(),
            Bytes::from_static(b"v"),
        )
        .await
        .unwrap();

    let collected = collect_stream(cached.scan_prefix_stream(Bytes::from_static(b"pfxA"), 3))
        .await
        .unwrap();

    // Exactly the 7 pfxA keys, in ascending order.
    assert_eq!(collected.len(), 7);
    for w in collected.windows(2) {
        assert!(
            w[0].0 < w[1].0,
            "scan_prefix_stream must yield ascending key order"
        );
    }
    for (k, _) in &collected {
        assert!(
            k.starts_with(b"pfxA"),
            "no non-matching keys may leak past the prefix boundary"
        );
    }
}

/// `scan_prefix_stream` early-termination: a consumer that drops the
/// stream after the first batch must only pay for that batch. We
/// confirm the stream yields a correct first batch and stops cleanly
/// when the consumer stops polling.
#[tokio::test]
async fn test_scan_prefix_stream_early_termination_first_batch() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

    // Seed 100 matching keys.
    for i in 0..100u32 {
        cached
            .set(
                Bytes::from(format!("pfx-{:04}", i)).into(),
                Bytes::from_static(b"v"),
            )
            .await
            .unwrap();
    }

    let mut stream = cached.scan_prefix_stream(Bytes::from_static(b"pfx"), 10);
    // Consume ONLY the first batch, then drop the stream.
    let first = stream.next().await.expect("first batch").unwrap();
    assert_eq!(first.len(), 10);
    // The first 10 keys in ascending order.
    for w in first.windows(2) {
        assert!(w[0].0 < w[1].0);
    }
    assert_eq!(first.first().unwrap().0, Bytes::from("pfx-0000"));
    // Dropping the stream here must not panic or hang.
    drop(stream);
}

/// `scan_prefix_stream` on a prefix with NO matches yields nothing
/// (no empty batches, no infinite loop on the resume cursor).
#[tokio::test]
async fn test_scan_prefix_stream_no_matches() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();

    cached
        .set(Bytes::from_static(b"aaa").into(), Bytes::from_static(b"v"))
        .await
        .unwrap();

    let collected = collect_stream(cached.scan_prefix_stream(Bytes::from_static(b"zzz"), 10))
        .await
        .unwrap();
    assert!(collected.is_empty());
}
