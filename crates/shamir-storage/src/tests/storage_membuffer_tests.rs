#![allow(deprecated)]

use crate::error::DbError;
use crate::storage_cached::CachedStore;
use crate::storage_in_memory::{InMemoryRepo, InMemoryStore};
use crate::storage_membuffer::{MemBufferConfig, MemBufferStore};
use crate::tests::types_tests::run_batch_store_tests;
use crate::types::{fully_unwrap_store, RecordKey, Repo, Store};
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
    let seed_key = RecordKey::from_slice(b"seed-key");
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
    let seed_key = RecordKey::from_slice(b"chain-key");
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

// ============================================================================
// Audit finding 2.3 (task #530): merge-overlay scans — a scan must NOT drain
// the dirty buffer to disk first (read-triggered write amplification). It must
// still return CORRECT results: entries only in the dirty overlay are visible,
// tombstoned keys are excluded, ordering is preserved, and dirty is left
// UNFLUSHED after the scan.
// ============================================================================

/// Config with a long flush interval. `@fl` review nit (task #530): this
/// alone does NOT stop the background flusher from firing — every
/// `set`/`remove` calls `notify.notify_one()`, and the flusher's `select!`
/// wakes on that notification regardless of `flush_interval_ms`. The `!
/// is_dirty_empty()` assertions below only hold because these tests run on
/// the current-thread `#[tokio::test]` runtime, where every `.await` in the
/// test body resolves `Ready` without ever yielding to the spawned flusher
/// task — so the flusher provably never gets scheduled during the test. This
/// is intentionally test-only groundedness, not a runtime-flavor-independent
/// guarantee; do not port these tests to a multi-thread runtime without
/// re-deriving why the flusher still can't preempt them.
fn no_flush_config() -> MemBufferConfig {
    MemBufferConfig {
        max_bytes: 64 * 1024 * 1024,
        max_entries: 1_000_000,
        ttl_ms: None,
        flush_interval_ms: 600_000,
        flush_batch_size: 256,
    }
}

fn rk(b: &[u8]) -> RecordKey {
    RecordKey::from_slice(b)
}

async fn collect_stream(
    mut s: crate::types::RecordStream,
) -> Vec<(RecordKey, Bytes)> {
    use futures::StreamExt;
    let mut out = Vec::new();
    while let Some(batch) = s.next().await {
        out.extend(batch.unwrap());
    }
    out
}

#[tokio::test]
async fn iter_stream_merges_overlay_without_draining() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    // Seed inner directly with a durable key.
    inner_store
        .set(rk(b"k1"), Bytes::from_static(b"inner1"))
        .await
        .unwrap();

    let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), no_flush_config()));
    // Overlay-only key (never flushed), an override of the inner key, and a
    // tombstone masking a (would-be) inner key.
    buffered
        .set(rk(b"k2"), Bytes::from_static(b"overlay2"))
        .await
        .unwrap();
    buffered
        .set(rk(b"k1"), Bytes::from_static(b"overlay1"))
        .await
        .unwrap();
    // Seed a stale inner key that the overlay tombstones.
    inner_store
        .set(rk(b"k3"), Bytes::from_static(b"stale3"))
        .await
        .unwrap();
    buffered.remove(rk(b"k3")).await.unwrap();

    let got = collect_stream(buffered.iter_stream(8)).await;
    let map: std::collections::BTreeMap<Vec<u8>, Vec<u8>> = got
        .iter()
        .map(|(k, v)| (k.as_ref().to_vec(), v.as_ref().to_vec()))
        .collect();

    // k1 → overlay wins over inner; k2 → overlay-only visible; k3 → tombstoned.
    assert_eq!(map.get(b"k1".as_ref()).unwrap(), b"overlay1");
    assert_eq!(map.get(b"k2".as_ref()).unwrap(), b"overlay2");
    assert!(!map.contains_key(b"k3".as_ref()), "tombstoned key must be excluded");
    assert_eq!(map.len(), 2, "exactly k1,k2 visible: {map:?}");

    // The scan must NOT have drained the dirty buffer to disk.
    assert!(
        !buffered.is_dirty_empty(),
        "scan drained the dirty buffer (write amplification) — merge-overlay expected"
    );
}

#[tokio::test]
async fn scan_prefix_stream_merges_overlay_sorted_and_in_prefix() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    inner_store
        .set(rk(b"pre:b"), Bytes::from_static(b"inner_b"))
        .await
        .unwrap();
    inner_store
        .set(rk(b"pre:d"), Bytes::from_static(b"inner_d"))
        .await
        .unwrap();
    // Out-of-prefix inner key — must never surface under the prefix scan.
    inner_store
        .set(rk(b"zzz"), Bytes::from_static(b"other"))
        .await
        .unwrap();

    let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), no_flush_config()));
    // Overlay: an in-prefix override, an in-prefix new key, an out-of-prefix
    // key (must be filtered), and a tombstone on an inner in-prefix key.
    buffered
        .set(rk(b"pre:a"), Bytes::from_static(b"ov_a"))
        .await
        .unwrap();
    buffered
        .set(rk(b"pre:b"), Bytes::from_static(b"ov_b"))
        .await
        .unwrap();
    buffered
        .set(rk(b"zzz2"), Bytes::from_static(b"ov_out"))
        .await
        .unwrap();
    buffered.remove(rk(b"pre:d")).await.unwrap();

    let got = collect_stream(buffered.scan_prefix_stream(Bytes::from_static(b"pre:"), 2)).await;

    // Ordering must be ascending lexicographic (callers rely on it).
    let keys: Vec<Vec<u8>> = got.iter().map(|(k, _)| k.as_ref().to_vec()).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "prefix scan must yield ascending order");

    let map: std::collections::BTreeMap<Vec<u8>, Vec<u8>> = got
        .iter()
        .map(|(k, v)| (k.as_ref().to_vec(), v.as_ref().to_vec()))
        .collect();
    assert_eq!(map.get(b"pre:a".as_ref()).unwrap(), b"ov_a");
    assert_eq!(map.get(b"pre:b".as_ref()).unwrap(), b"ov_b"); // overlay wins
    assert!(!map.contains_key(b"pre:d".as_ref()), "tombstone excludes pre:d");
    assert!(!map.iter().any(|(k, _)| !k.starts_with(b"pre:")), "no out-of-prefix keys");
    assert_eq!(map.len(), 2, "pre:a, pre:b only: {map:?}");

    assert!(!buffered.is_dirty_empty(), "prefix scan must not drain dirty");
}

#[tokio::test]
async fn iter_range_stream_merges_overlay_ascending() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    for (k, v) in [(b"a" as &[u8], "ia"), (b"c", "ic"), (b"e", "ie"), (b"g", "ig")] {
        inner_store
            .set(rk(k), Bytes::copy_from_slice(v.as_bytes()))
            .await
            .unwrap();
    }
    let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), no_flush_config()));
    // Overlay keys interleaved in the range, plus a tombstone.
    buffered.set(rk(b"b"), Bytes::from_static(b"ob")).await.unwrap();
    buffered.set(rk(b"d"), Bytes::from_static(b"od")).await.unwrap();
    buffered.set(rk(b"c"), Bytes::from_static(b"oc")).await.unwrap(); // override
    buffered.remove(rk(b"e")).await.unwrap(); // tombstone

    // Range [b ..= f].
    let got = collect_stream(buffered.iter_range_stream(
        Some(Bytes::from_static(b"b")),
        Some(Bytes::from_static(b"f")),
        2,
    ))
    .await;
    let keys: Vec<Vec<u8>> = got.iter().map(|(k, _)| k.as_ref().to_vec()).collect();
    // Expect ascending: b, c, d (a excluded < lo; e tombstoned; g excluded > hi).
    assert_eq!(
        keys,
        vec![b"b".to_vec(), b"c".to_vec(), b"d".to_vec()],
        "range scan must be ascending + correctly merged"
    );
    let map: std::collections::BTreeMap<Vec<u8>, Vec<u8>> = got
        .iter()
        .map(|(k, v)| (k.as_ref().to_vec(), v.as_ref().to_vec()))
        .collect();
    assert_eq!(map.get(b"c".as_ref()).unwrap(), b"oc", "overlay override wins");
    assert!(!buffered.is_dirty_empty(), "range scan must not drain dirty");
}

#[tokio::test]
async fn iter_range_stream_reverse_merges_overlay_descending() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    for (k, v) in [(b"a" as &[u8], "ia"), (b"c", "ic"), (b"e", "ie"), (b"g", "ig")] {
        inner_store
            .set(rk(k), Bytes::copy_from_slice(v.as_bytes()))
            .await
            .unwrap();
    }
    let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), no_flush_config()));
    buffered.set(rk(b"b"), Bytes::from_static(b"ob")).await.unwrap();
    buffered.set(rk(b"d"), Bytes::from_static(b"od")).await.unwrap();
    buffered.set(rk(b"c"), Bytes::from_static(b"oc")).await.unwrap();
    buffered.remove(rk(b"e")).await.unwrap();

    let got = collect_stream(buffered.iter_range_stream_reverse(
        Some(Bytes::from_static(b"b")),
        Some(Bytes::from_static(b"f")),
        2,
    ))
    .await;
    let keys: Vec<Vec<u8>> = got.iter().map(|(k, _)| k.as_ref().to_vec()).collect();
    // Descending: d, c, b (a < lo; e tombstoned; g > hi).
    assert_eq!(
        keys,
        vec![b"d".to_vec(), b"c".to_vec(), b"b".to_vec()],
        "reverse range scan must be descending + correctly merged"
    );
    assert!(!buffered.is_dirty_empty(), "reverse range scan must not drain dirty");
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
}

// ============================================================================
// Task #535: `dirty_nonempty` clear-race — `drain_once` must not mask an
// already-ACKed write. The naive `is_empty()` → `store(false)` sequence has a
// check-then-act gap: a writer's `dirty.insert` (preceded by its own
// `store(true)`) can land between the check and the store, leaving a real entry
// in `dirty` while the sentinel wrongly reads `false`. Every subsequent `get()`
// then skips the dirty probe via that false sentinel and stale-misses the write.
//
// The regression is driven deterministically (NO sleep-based timing) via a
// `#[cfg(test)]` `ClearRaceHook` fired inside `drain_once` at the exact clear
// window (after the `is_empty()` observation, BEFORE the `store(false)` that
// follows it): the hook injects the racing writer insert into that gap.
// Pre-fix, the sentinel ends up `false` with the key still in `dirty` and NOT
// yet in `inner`, so the immediate `get()` stale-misses it (NotFound).
// Post-fix, the verify-after-clear re-check observes the raced-in entry and
// restores the sentinel to `true`, so the `get()` finds it. (A second,
// narrower gap in that same fix — a writer that publishes `store(true)` then
// stalls across an `.await` before its own `dirty.insert()` completes, e.g.
// mid-loop in `insert_many`/`set_many`/`remove_many` — is closed separately
// by the writer-side republish-after-insert; this test targets the
// insert-observable-by-the-re-check case specifically.)
// ============================================================================
mod clear_race_535 {
    use super::*;
    use crate::membuffer_clear_race_hook::ClearRaceHook;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, Weak};

    #[tokio::test]
    async fn drain_clear_race_does_not_mask_acked_write() {
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();

        // The racing key K: injected by the hook into `dirty` inside the clear
        // window. It is NEVER written to `inner` up front, so if the sentinel is
        // wrongly cleared and `get(K)` skips the dirty probe, `inner` also
        // misses → NotFound. That NotFound is exactly the masked write.
        let racing_key = rk(b"racing-key-535");
        let racing_val = Bytes::from_static(b"racing-value-535");

        // Late-bound slot so the hook can reach the store built AFTER the hook.
        let store_slot: Arc<Mutex<Weak<MemBufferStore>>> =
            Arc::new(Mutex::new(Weak::new()));
        // Fire exactly ONCE — the drain-loop is not used here (single
        // `drain_once` pass), but guard defensively so the injected K is not
        // re-inserted on any later drain.
        let fired = Arc::new(AtomicBool::new(false));

        let hook = {
            let store_slot = Arc::clone(&store_slot);
            let fired = Arc::clone(&fired);
            let racing_key = racing_key.clone();
            let racing_val = racing_val.clone();
            ClearRaceHook::install(Arc::new(move || {
                if fired.swap(true, Ordering::SeqCst) {
                    return;
                }
                // Simulate a writer whose `store(true)` + `dirty.insert` raced
                // into the clear gap: publish the sentinel, then land the entry.
                if let Some(store) = store_slot.lock().unwrap().upgrade() {
                    store.inject_racing_dirty_write(racing_key.clone(), racing_val.clone());
                }
            }))
        };

        let buffered = Arc::new(MemBufferStore::new_with_clear_race_hook(
            inner_store,
            no_flush_config(),
            hook,
        ));
        *store_slot.lock().unwrap() = Arc::downgrade(&buffered);

        // Prime `dirty` with an unrelated key J so the single `drain_once`
        // actually flushes something and reaches the clear window.
        let jkey = buffered.insert(Bytes::from_static(b"prime-j")).await.unwrap();

        // ONE drain pass: flushes J → `dirty` observed empty → `store(false)` →
        // hook injects K into the gap → verify-after-clear must restore the
        // sentinel (post-fix).
        let drained = buffered.drain_once_for_test(usize::MAX).await.unwrap();
        assert_eq!(drained, 1, "the single drain should have flushed only J");

        // The racing key K is now in `dirty` and NOT in `inner`. The sentinel
        // must be TRUE — otherwise `get(K)` (and every overlay scan) masks it.
        assert!(
            buffered.dirty_nonempty_flag(),
            "clear-race masked an ACKed write: dirty holds K but dirty_nonempty=false"
        );

        // End-to-end: the ACKed write must be visible immediately. Pre-fix this
        // returns NotFound (fast-path skips the dirty probe on the false
        // sentinel, and inner never had K).
        let got = buffered.get(racing_key).await.unwrap();
        assert_eq!(
            got.as_ref(),
            b"racing-value-535",
            "racing write must be visible after the clear-race window"
        );

        // Sanity: J did reach inner (the drain flushed it).
        let jkey_owned = jkey;
        assert!(
            buffered.get(jkey_owned).await.is_ok(),
            "primed key J should have been flushed to inner"
        );
    }
}

// ============================================================================
// Task #535 round 2: the narrower stall-across-`.await` gap an `@fl`
// adversarial pass found in round 1's fix. A writer's `dirty_nonempty.
// store(true)` published BEFORE its `dirty.insert()` does not help if the
// writer stalls (an `.await` yield) between the two AND a concurrent
// `drain_once` runs its ENTIRE clear-and-verify sequence inside that stall —
// neither of `drain_once`'s two `is_empty()` checks observes the not-yet-
// landed entry, so round 1's verify-after-clear has nothing to restore, and
// the writer's later insert lands with the sentinel stuck `false`. Real,
// non-hypothetical for `insert_many`/`set_many`/`remove_many`, whose per-item
// loop yields at `cache.insert(...).await` every iteration while a single
// `store(true)` was hoisted once before the loop.
//
// Closed by republishing `dirty_nonempty.store(true, Release)` immediately
// after EACH per-item `dirty.insert()`, not just once before the loop (see
// `insert_many`'s loop body). This test drives the exact interleaving via a
// `BatchInsertPauseHook` parked between `insert_many`'s first and second
// iteration.
// ============================================================================
mod batch_insert_republish_535 {
    use super::*;
    use crate::membuffer_clear_race_hook::BatchInsertPauseHook;

    #[tokio::test]
    async fn insert_many_second_item_survives_drain_racing_between_iterations() {
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();

        let buffered = Arc::new(MemBufferStore::new(inner_store, no_flush_config()));
        // Pre-empt the background flusher BEFORE any `.await` — this test
        // spawns `insert_many` and genuinely suspends on a `Notify`, which
        // (unlike the round-1 test above) gives the runtime its first real
        // chance to schedule the separately-spawned flusher task. Without
        // this, the flusher can legitimately race the manually-driven
        // `drain_once_for_test` below and flush v1 to `inner` on its own,
        // making the test pass regardless of whether the round-2 fix is
        // present. See `disable_background_flusher_for_test`'s doc comment.
        buffered.disable_background_flusher_for_test();

        let hook = Arc::new(BatchInsertPauseHook::new());
        buffered.set_batch_pause_hook(Some(Arc::clone(&hook)));

        // Spawn insert_many([v0, v1]) — it will park after v0's iteration
        // (dirty.insert + round-2 republish + cache write all done for v0),
        // before v1's dirty.insert runs.
        let buffered_writer = Arc::clone(&buffered);
        let insert_task = tokio::spawn(async move {
            buffered_writer
                .insert_many(vec![
                    Bytes::from_static(b"v0-535"),
                    Bytes::from_static(b"v1-535"),
                ])
                .await
        });

        hook.wait_until_parked().await;

        // While the writer is parked (v1 NOT in dirty yet), run a real
        // drain_once: it flushes v0 (the only entry in dirty right now) to
        // inner, removes it via remove_if, observes dirty EMPTY (v1 isn't
        // there yet) on BOTH the initial check and round 1's verify-after-
        // clear re-check, and correctly clears the sentinel to `false` — this
        // is NOT a bug at this point, dirty genuinely is empty.
        let drained = buffered.drain_once_for_test(usize::MAX).await.unwrap();
        assert_eq!(drained, 1, "the racing drain should have flushed only v0");
        assert!(
            !buffered.dirty_nonempty_flag(),
            "sentinel correctly clear here — dirty is genuinely empty before v1 lands"
        );

        // Release the writer — it now inserts v1 into dirty. Pre-round-2-fix
        // this leaves the sentinel stuck `false` (masking v1); post-fix, the
        // per-item republish immediately after v1's dirty.insert restores it.
        //
        // The background flusher was disabled above (before its first poll),
        // so nothing else can touch `dirty`/`dirty_nonempty` concurrently
        // here — the assertion below is deterministic, not a race with any
        // other drainer.
        hook.release();
        let keys = insert_task
            .await
            .unwrap()
            .expect("insert_many must succeed");
        assert_eq!(keys.len(), 2);
        let v1_key = keys[1].clone();

        assert!(
            buffered.dirty_nonempty_flag(),
            "round-2 gap: v1 landed in dirty after the racing drain cleared the \
             sentinel, but nothing republished `true` for it"
        );

        // Force-evict v1 from the moka cache: `get()` checks the cache
        // FIRST and only falls through to the `dirty_nonempty`-gated `dirty`
        // probe on a cache miss. `insert_many` also writes v1 into cache, so
        // without this eviction `get()` would trivially cache-hit regardless
        // of whether the sentinel is correct — this line is what actually
        // exercises the vulnerable fast-path. With the flusher disabled and
        // no other drainer running, `inner` provably lacks v1 at this point,
        // so this deterministically forces the code through the
        // dirty-probe path the fix protects.
        buffered.evict_from_cache_for_test(&v1_key).await;

        // End-to-end: v1 must be visible immediately (not masked, not
        // stale-missed via the false sentinel skipping the dirty probe).
        // Pre-round-2-fix: v1 sits in `dirty`, the sentinel is wrongly
        // `false` (the racing drain cleared it before v1 landed), and no
        // legitimate flush has necessarily happened yet — `get()` fast-path
        // skips the dirty probe and falls through to `inner`, which also
        // lacks v1 → NotFound (the masked write).
        let got = buffered.get(v1_key).await.unwrap();
        assert_eq!(got.as_ref(), b"v1-535");

        // v0 reached inner via the racing drain.
        let v0_key = keys[0].clone();
        assert!(
            buffered.get(v0_key).await.is_ok(),
            "v0 should have been flushed to inner by the racing drain"
        );
    }
}
