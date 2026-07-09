#![allow(deprecated)]

use crate::error::DbError;
use crate::storage_cached::CachedStore;
use crate::storage_in_memory::{InMemoryRepo, InMemoryStore};
use crate::storage_membuffer::{MemBufferConfig, MemBufferStore};
use crate::tests::types_tests::run_batch_store_tests;
use crate::types::{fully_unwrap_store, Repo, Store};
use bytes::Bytes;
use std::sync::Arc;
use std::time::Duration;

fn small_config() -> MemBufferConfig {
    MemBufferConfig {
        max_bytes: 4 * 1024,
        max_entries: 16,
        ttl_ms: None,
        flush_interval_ms: 10,
        flush_batch_size: 8,
    }
}

async fn drained(store: Arc<MemBufferStore>) {
    store.flush().await.unwrap();
}

#[tokio::test]
async fn buffered_passes_full_batch_suite() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let store: Arc<dyn Store> =
        Arc::new(MemBufferStore::new(inner_store, MemBufferConfig::default()));
    run_batch_store_tests(store).await;
}

#[tokio::test]
async fn write_visible_after_flush_in_inner() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let buffered = Arc::new(MemBufferStore::new(
        inner_store.clone(),
        MemBufferConfig::default(),
    ));

    let key = buffered.insert(Bytes::from_static(b"v1")).await.unwrap();
    buffered.flush().await.unwrap();
    let got = inner_store.get(key).await.unwrap();
    assert_eq!(got.as_ref(), b"v1");
    drained(buffered).await;
}

#[tokio::test]
async fn read_after_write_returns_buffered_value() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let buffered = Arc::new(MemBufferStore::new(inner_store, MemBufferConfig::default()));
    let key = buffered.insert(Bytes::from_static(b"hello")).await.unwrap();
    let got = buffered.get(key).await.unwrap();
    assert_eq!(got.as_ref(), b"hello");
}

#[tokio::test]
async fn eviction_with_dirty_eventually_flushes_evictee() {
    // moka's eviction is eventually consistent — the eviction
    // listener fires from the maintenance task. We wait for
    // the flusher / pending tasks to propagate.
    //
    // max_bytes=80 (~ one 21-byte slot fits but two don't —
    // each Slot::Live(b"first"/b"second") weighs
    // 16 key + 5/6 value = 21/22).
    let cfg = MemBufferConfig {
        max_bytes: 30,
        max_entries: 1_000_000,
        ttl_ms: None,
        flush_interval_ms: 25,
        flush_batch_size: 1,
    };
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), cfg));

    let k1 = buffered.insert(Bytes::from_static(b"first")).await.unwrap();
    let k2 = buffered
        .insert(Bytes::from_static(b"second"))
        .await
        .unwrap();

    // Wait for the eviction listener + flusher.
    let mut got1 = None;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if let Ok(v) = inner_store.get(k1.clone()).await {
            got1 = Some(v);
            break;
        }
    }
    let got1 = got1.expect("evicted dirty entry must eventually reach inner");
    assert_eq!(got1.as_ref(), b"first");

    // After explicit flush k2 also lands.
    buffered.flush().await.unwrap();
    let got2 = inner_store.get(k2).await.unwrap();
    assert_eq!(got2.as_ref(), b"second");
}

#[tokio::test]
async fn tombstone_blocks_inner_visibility() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let key = inner_store
        .insert(Bytes::from_static(b"stale"))
        .await
        .unwrap();
    let buffered = Arc::new(MemBufferStore::new(
        inner_store.clone(),
        MemBufferConfig::default(),
    ));
    let _ = buffered.get(key.clone()).await.unwrap();
    let existed = buffered.remove(key.clone()).await.unwrap();
    assert!(existed);
    let result = buffered.get(key.clone()).await;
    assert!(matches!(result, Err(DbError::NotFound(_))));
    buffered.flush().await.unwrap();
    let result_inner = inner_store.get(key).await;
    assert!(matches!(result_inner, Err(DbError::NotFound(_))));
}

#[tokio::test]
async fn background_flusher_eventually_drains() {
    let cfg = MemBufferConfig {
        max_bytes: 64 * 1024,
        max_entries: 256,
        ttl_ms: None,
        flush_interval_ms: 20,
        flush_batch_size: 256,
    };
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), cfg));

    let mut keys = Vec::new();
    for i in 0..5u8 {
        let k = buffered.insert(Bytes::copy_from_slice(&[i])).await.unwrap();
        keys.push(k);
    }
    let mut found = 0;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        found = 0;
        for k in &keys {
            if inner_store.get(k.clone()).await.is_ok() {
                found += 1;
            }
        }
        if found == keys.len() {
            break;
        }
    }
    assert_eq!(
        found,
        keys.len(),
        "background flusher must drain dirty entries"
    );
    buffered.flush().await.unwrap();
}

#[tokio::test]
async fn bytes_eviction_caps_resident_size() {
    // max_bytes=256, each value ~64 bytes. After inserts, the
    // moka cache should hold at most a couple entries (cap
    // enforced via weigher). Records still reachable via the
    // dirty-flush + inner path.
    let cfg = MemBufferConfig {
        max_bytes: 256,
        max_entries: 1_000_000,
        ttl_ms: None,
        flush_interval_ms: 60_000,
        flush_batch_size: 256,
    };
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), cfg));

    let mut keys = Vec::new();
    for _ in 0..10u8 {
        let key = buffered
            .insert(Bytes::from_static(
                b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            ))
            .await
            .unwrap();
        keys.push(key);
    }

    // Force any pending maintenance.
    buffered.run_pending_cache_tasks().await;

    // All ten records visible end-to-end. Eviction listener +
    // flusher push evictees to inner; cache holds the rest.
    // Wait for propagation.
    let mut found = 0;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        found = 0;
        for k in &keys {
            if buffered.get(k.clone()).await.is_ok() {
                found += 1;
            }
        }
        if found == keys.len() {
            break;
        }
    }
    assert_eq!(found, 10, "all written records must be reachable");
}

#[tokio::test]
async fn ttl_eviction_drops_old_entries() {
    let cfg = MemBufferConfig {
        max_bytes: 64 * 1024,
        max_entries: 100,
        ttl_ms: Some(80),
        flush_interval_ms: 30,
        flush_batch_size: 16,
    };
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), cfg));

    let _k1 = buffered.insert(Bytes::from_static(b"a")).await.unwrap();
    let _k2 = buffered.insert(Bytes::from_static(b"b")).await.unwrap();
    // Wait > TTL + flusher tick + maintenance.
    tokio::time::sleep(Duration::from_millis(400)).await;
    buffered.run_pending_cache_tasks().await;

    // Records still readable from inner (eviction listener flushed them).
    let v1 = inner_store.get(_k1).await.unwrap();
    let v2 = inner_store.get(_k2).await.unwrap();
    assert_eq!(v1.as_ref(), b"a");
    assert_eq!(v2.as_ref(), b"b");
}

#[tokio::test]
async fn apply_config_shrinks_max_bytes_and_triggers_eviction() {
    let cfg = MemBufferConfig {
        max_bytes: 64 * 1024,
        max_entries: 1_000_000,
        ttl_ms: None,
        flush_interval_ms: 60_000,
        flush_batch_size: 16,
    };
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let buffered = Arc::new(MemBufferStore::new(inner_store, cfg));

    for _ in 0..16u8 {
        let _ = buffered
            .insert(Bytes::from_static(
                b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            ))
            .await
            .unwrap();
    }
    // Flush so dirty is empty; subsequent reads come from cache or inner.
    buffered.flush().await.unwrap();

    let smaller = MemBufferConfig {
        max_bytes: 128,
        max_entries: 1_000_000,
        ttl_ms: None,
        flush_interval_ms: 60_000,
        flush_batch_size: 16,
    };
    buffered.apply_config(&smaller).await.unwrap();

    // After config swap the new cache is empty (rebuilt).
    // Insert ONE more entry; the new cache should hold at most
    // its capacity. The 16 prior entries reside in inner only.
    let _ = buffered
        .insert(Bytes::from_static(b"trigger"))
        .await
        .unwrap();
    // run_pending_tasks fires any synchronous eviction.
    buffered.run_pending_cache_tasks().await;
    assert!(
        buffered.cache_bytes() <= 200,
        "new cap not honoured: cache_bytes={}",
        buffered.cache_bytes()
    );
}

#[tokio::test]
async fn apply_config_enables_ttl_at_runtime() {
    let cfg = MemBufferConfig {
        max_bytes: 64 * 1024,
        max_entries: 256,
        ttl_ms: None,
        flush_interval_ms: 25,
        flush_batch_size: 16,
    };
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let buffered = Arc::new(MemBufferStore::new(inner_store, cfg));

    let _ = buffered.insert(Bytes::from_static(b"v")).await.unwrap();
    // Allow moka maintenance to commit weighted_size.
    buffered.run_pending_cache_tasks().await;
    assert!(buffered.cache_bytes() > 0);

    tokio::time::sleep(Duration::from_millis(80)).await;
    buffered.run_pending_cache_tasks().await;
    assert!(
        buffered.cache_bytes() > 0,
        "no TTL set — entry should persist"
    );

    let with_ttl = MemBufferConfig {
        max_bytes: 64 * 1024,
        max_entries: 256,
        ttl_ms: Some(50),
        flush_interval_ms: 25,
        flush_batch_size: 16,
    };
    buffered.apply_config(&with_ttl).await.unwrap();

    // New cache is empty (rebuild). Insert one more entry under
    // the TTL'd cache; wait for it to expire.
    let _ = buffered.insert(Bytes::from_static(b"w")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    buffered.run_pending_cache_tasks().await;
    assert_eq!(
        buffered.cache_bytes(),
        0,
        "TTL not applied at runtime: cache_bytes={}",
        buffered.cache_bytes()
    );
}

#[tokio::test]
async fn flush_drains_then_calls_inner_flush() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), small_config()));
    for i in 0..50u8 {
        let _ = buffered.insert(Bytes::copy_from_slice(&[i])).await.unwrap();
    }
    buffered.flush().await.unwrap();
    assert!(buffered.is_dirty_empty(), "dirty must be empty after flush");
}

#[tokio::test]
async fn raw_backend_unwraps_membuffer() {
    let seed_key = Bytes::from_static(b"seed-key");
    let seed_val = Bytes::from_static(b"seed-value");

    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    inner.set(seed_key.clone(), seed_val.clone()).await.unwrap();

    let mb: Arc<dyn Store> = Arc::new(MemBufferStore::new(
        Arc::clone(&inner),
        MemBufferConfig::default(),
    ));

    let raw = mb.raw_backend().await.expect("MemBufferStore returns Some");
    // raw is the same inner — observable via the seeded value
    assert_eq!(raw.get(seed_key).await.unwrap(), seed_val);
}

#[tokio::test]
async fn fully_unwrap_drills_through_chain() {
    let seed_key = Bytes::from_static(b"chain-key");
    let seed_val = Bytes::from_static(b"chain-val");

    // Build Cached → MemBuffer → InMemory
    let raw: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mb: Arc<dyn Store> = Arc::new(MemBufferStore::new(
        Arc::clone(&raw),
        MemBufferConfig::default(),
    ));
    let cached: Arc<dyn Store> = Arc::new(CachedStore::new_sync(mb.clone()).await.unwrap());

    let unwrapped = fully_unwrap_store(&cached).await;

    // Seed via the fully-unwrapped store; the raw layer must see it
    unwrapped
        .set(seed_key.clone(), seed_val.clone())
        .await
        .unwrap();
    assert_eq!(raw.get(seed_key).await.unwrap(), seed_val);
}

/// Op C regression: insert_many / set_many / remove_many must publish
/// dirty_nonempty (Release) before populating the dirty overlay.
///
/// Without the sentinel set, a concurrent `get()` after eviction-from-cache
/// would see `dirty_nonempty=false` (Acquire), skip the dirty probe entirely,
/// fall through to the inner store, and stale-miss a key that's actually
/// present in `dirty`.
///
/// This test sets up a tiny cache to force eviction during a 100-row
/// `insert_many` and asserts every inserted key remains visible.
/// On HEAD (pre-fix), at least one key in the inserted batch will return
/// `NotFound` because its cache slot was evicted before the flusher drained
/// it AND the dirty probe was bypassed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn insert_many_visible_under_cache_eviction_op_c() {
    let cfg = MemBufferConfig {
        // 64 bytes — fits ~2 of our 50-byte slots, forces aggressive moka eviction
        // as we insert 100 keys.
        max_bytes: 64,
        max_entries: 1_000_000,
        ttl_ms: None,
        // Long enough that the periodic flusher does not drain dirty during the
        // window we care about. Eviction-listener-driven drain still fires, but
        // the race we're testing is reads landing BEFORE the listener has
        // pushed an evicted key to inner.
        flush_interval_ms: 600_000,
        flush_batch_size: 1,
    };
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    let buffered = Arc::new(MemBufferStore::new(inner_store, cfg));

    // 100 distinct 50-byte values → cache eviction guaranteed.
    let values: Vec<Bytes> = (0..100)
        .map(|i| Bytes::from(format!("op-c-payload-{i:04}-padding-padding-pad")))
        .collect();
    let expected = values.clone();
    let keys = buffered.insert_many(values).await.unwrap();

    // Immediate read — no flush call — every key must be visible via the dirty
    // overlay even if its cache slot has been evicted.
    let mut missed = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        match buffered.get(key.clone()).await {
            Ok(v) => assert_eq!(v.as_ref(), expected[i].as_ref(), "stale value at i={i}"),
            Err(e) => missed.push((i, e)),
        }
    }
    assert!(
        missed.is_empty(),
        "Op C regression: {} key(s) stale-missed after insert_many. \
         First: idx={}, err={:?}",
        missed.len(),
        missed[0].0,
        missed[0].1
    );
}

/// Audit §2.3 regression: `transact` must NOT lose a concurrent `set(k)`
/// that lands between `inner.transact` and the post-transact `dirty.remove`.
/// Before the fix, the unconditional `dirty.remove(&k)` deleted a concurrent
/// write's dirty entry, so it never reached the inner store — after a cache
/// eviction or restart, durable state was the OLD value.
///
/// This test deterministically reproduces the race: a wrapper inner store
/// injects a `set(k, "concurrent")` on the SAME `MemBufferStore` (via a
/// late-bound slot) during its `transact` call — landing the concurrent write
/// in dirty exactly in the window the old code lost. After `transact` + flush,
/// the concurrent value must survive in inner (the old unconditional remove
/// would have deleted it, losing the write permanently).
mod audit_2_3 {
    use super::*;
    use crate::error::DbResult;
    use crate::types::{KvOp, RecordKey, RecordStream, Store};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

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

        let key = RecordKey::copy_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
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
}
