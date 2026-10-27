#![allow(deprecated)]

use crate::storage_in_memory::InMemoryRepo;
use crate::storage_membuffer::{MemBufferConfig, MemBufferStore};
use crate::types::{RecordKey, Repo, Store};
use bytes::Bytes;
use futures::StreamExt;
use std::sync::Arc;

use super::common::{no_flush_config, rk};
async fn collect_stream(mut s: crate::types::RecordStream) -> Vec<(RecordKey, Bytes)> {
    use futures::StreamExt;
    let mut out = Vec::new();
    while let Some(batch) = s.next().await {
        out.extend(batch.unwrap());
    }
    out
}

#[tokio::test]
async fn rv_storage_5e99bede_membuffer_zero_batch_is_empty_with_dirty_overlay() {
    let repo = InMemoryRepo::new();
    let inner = repo.store_get("zero-batch").await.unwrap();
    inner
        .set(rk(b"persisted"), Bytes::from_static(b"inner"))
        .await
        .unwrap();
    let buffered = MemBufferStore::new(inner, no_flush_config());
    buffered
        .set(rk(b"dirty"), Bytes::from_static(b"overlay"))
        .await
        .unwrap();
    assert!(buffered.iter_stream(0).next().await.is_none());
    assert!(buffered
        .scan_prefix_stream(Bytes::from_static(b"d"), 0)
        .next()
        .await
        .is_none());
    assert!(buffered
        .iter_range_stream(None, None, 0)
        .next()
        .await
        .is_none());
    assert!(buffered
        .iter_range_stream_reverse(None, None, 0)
        .next()
        .await
        .is_none());
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
    assert!(
        !map.contains_key(b"k3".as_ref()),
        "tombstoned key must be excluded"
    );
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
    assert!(
        !map.contains_key(b"pre:d".as_ref()),
        "tombstone excludes pre:d"
    );
    assert!(
        !map.iter().any(|(k, _)| !k.starts_with(b"pre:")),
        "no out-of-prefix keys"
    );
    assert_eq!(map.len(), 2, "pre:a, pre:b only: {map:?}");

    assert!(
        !buffered.is_dirty_empty(),
        "prefix scan must not drain dirty"
    );
}

#[tokio::test]
async fn iter_range_stream_merges_overlay_ascending() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    for (k, v) in [
        (b"a" as &[u8], "ia"),
        (b"c", "ic"),
        (b"e", "ie"),
        (b"g", "ig"),
    ] {
        inner_store
            .set(rk(k), Bytes::copy_from_slice(v.as_bytes()))
            .await
            .unwrap();
    }
    let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), no_flush_config()));
    // Overlay keys interleaved in the range, plus a tombstone.
    buffered
        .set(rk(b"b"), Bytes::from_static(b"ob"))
        .await
        .unwrap();
    buffered
        .set(rk(b"d"), Bytes::from_static(b"od"))
        .await
        .unwrap();
    buffered
        .set(rk(b"c"), Bytes::from_static(b"oc"))
        .await
        .unwrap(); // override
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
    assert_eq!(
        map.get(b"c".as_ref()).unwrap(),
        b"oc",
        "overlay override wins"
    );
    assert!(
        !buffered.is_dirty_empty(),
        "range scan must not drain dirty"
    );
}

#[tokio::test]
async fn iter_range_stream_reverse_merges_overlay_descending() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();
    for (k, v) in [
        (b"a" as &[u8], "ia"),
        (b"c", "ic"),
        (b"e", "ie"),
        (b"g", "ig"),
    ] {
        inner_store
            .set(rk(k), Bytes::copy_from_slice(v.as_bytes()))
            .await
            .unwrap();
    }
    let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), no_flush_config()));
    buffered
        .set(rk(b"b"), Bytes::from_static(b"ob"))
        .await
        .unwrap();
    buffered
        .set(rk(b"d"), Bytes::from_static(b"od"))
        .await
        .unwrap();
    buffered
        .set(rk(b"c"), Bytes::from_static(b"oc"))
        .await
        .unwrap();
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
    assert!(
        !buffered.is_dirty_empty(),
        "reverse range scan must not drain dirty"
    );
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
