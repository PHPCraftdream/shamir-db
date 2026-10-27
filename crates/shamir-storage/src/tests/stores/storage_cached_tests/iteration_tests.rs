#![allow(deprecated)]

use crate::error::DbResult;
use crate::storage_cached::CachedStore;
use crate::storage_in_memory::InMemoryStore;
use crate::tests::types_tests::collect_stream;
use crate::types::{RecordKey, Store};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use futures::StreamExt;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
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

// ============================================================================
// WriteMode::Async ordering (taskId #616 pt.2, security-remediation brief 13)
// ============================================================================

/// A `Store` wrapper that artificially delays `set`/`remove` in INVERSE
/// call order: the Nth call sleeps for `(calls - N)` slice, so the FIRST
/// call submitted is the SLOWEST to actually reach `inner`, and later
/// calls finish sooner. This recreates the exact race the old
/// "independent `tokio::spawn` per write" implementation was exposed to:
/// if two background tasks for the same key are scheduled independently,
/// nothing stops the scheduler from completing the newer write before
/// the older one, leaving `inner` on a stale value.
///
/// Under the FIFO worker (the fix), submission order — not completion
/// timing — determines what lands in `inner` last, so this delay
/// ordering must NOT be able to reorder the final value.
struct DelayedStore {
    inner: Arc<dyn Store>,
    call_no: AtomicUsize,
    /// Total calls expected, so the first call can be given the longest
    /// delay (`total - 1`) and the last call the shortest (`0`).
    total_calls: usize,
    delay_step_ms: u64,
}

#[async_trait]
impl Store for DelayedStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let n = self.call_no.fetch_add(1, Ordering::SeqCst);
        let delay = self.total_calls.saturating_sub(n + 1) as u64 * self.delay_step_ms;
        if delay > 0 {
            sleep(Duration::from_millis(delay)).await;
        }
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>> + Send>>
    {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>> + Send>>
    {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

/// Regression test for the ordering bug the FIFO worker fixes: `set(k,
/// v1)` immediately followed by `set(k, v2)` on the SAME key, where the
/// backing store artificially finishes the FIRST call's work slower than
/// the SECOND's. Under the old "independent spawn per write" code this
/// would let `v1`'s background task land in `inner` AFTER `v2`'s,
/// leaving `inner` stuck on the stale `v1` post-`flush()`. Under the
/// FIFO worker, the queue processes `v1` fully before even starting
/// `v2` (one `.await` at a time), so submission order always wins
/// regardless of per-call latency.
#[tokio::test]
async fn test_async_mode_preserves_write_order_under_inverted_latency() {
    let plain_inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let delayed_inner = Arc::new(DelayedStore {
        inner: plain_inner.clone(),
        call_no: AtomicUsize::new(0),
        total_calls: 2,
        // First call sleeps ~40ms, second call sleeps 0ms — the older
        // write is the slower one to reach `inner`, worst case for the
        // old independent-spawn code.
        delay_step_ms: 40,
    }) as Arc<dyn Store>;

    let cached = CachedStore::new_async(delayed_inner).await.unwrap();

    let key = RecordKey::from_slice(b"ordering-key");
    let v1 = Bytes::from_static(b"v1-stale-if-buggy");
    let v2 = Bytes::from_static(b"v2-fresh");

    cached.set(key.clone(), v1.clone()).await.unwrap();
    cached.set(key.clone(), v2.clone()).await.unwrap();

    cached.flush().await.unwrap();

    // The cache already held v2 synchronously (upsert on every `set`).
    let from_cache = cached.get(key.clone()).await.unwrap();
    assert_eq!(from_cache, v2, "cache always holds the latest value");

    // The backing store — reached only through the background
    // worker — must ALSO hold v2, not v1, once flush() has drained the
    // ordered queue. A failure here (`v1`) proves the two writes
    // reached `inner` out of submission order.
    let from_inner = plain_inner.get(key).await.unwrap();
    assert_eq!(
        from_inner, v2,
        "FIFO worker must apply writes to inner in submission order, \
         even when an earlier write is artificially slower than a later one"
    );
}

/// Broader stress variant: many rapid alternating `set`/`remove` calls
/// on a SINGLE key. Whatever the final logical state is (from the
/// sequence of calls), that's what must be observed directly on
/// `inner` after `flush()` — proof the ordered queue doesn't let any
/// later write/removal be overtaken by an earlier one still in flight.
#[tokio::test]
async fn test_async_mode_many_writes_same_key_land_in_order() {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = CachedStore::new_async(inner.clone()).await.unwrap();

    let key = RecordKey::from_slice(b"hot-key");
    let mut last_was_set = false;
    let mut last_value = Bytes::new();

    for i in 0..50u32 {
        if i % 5 == 4 {
            // Every 5th op removes the key.
            cached.remove(key.clone()).await.unwrap();
            last_was_set = false;
        } else {
            let value = Bytes::from(format!("value-{:03}", i));
            cached.set(key.clone(), value.clone()).await.unwrap();
            last_was_set = true;
            last_value = value;
        }
    }

    cached.flush().await.unwrap();

    if last_was_set {
        let from_inner = inner.get(key).await.unwrap();
        assert_eq!(
            from_inner, last_value,
            "inner must reflect the LAST logical set, not an earlier one \
             that raced ahead of it"
        );
    } else {
        assert!(
            inner.get(key).await.is_err(),
            "inner must reflect the LAST logical remove"
        );
    }
}

// ============================================================================
// #1082: flush() surfaces background write/remove failures (previously
// silently lost — the caller had no way to distinguish a failed background
// write from a successful one, since flush() always returned Ok(())).
// ============================================================================

#[tokio::test]
async fn rv_storage_d00d27de_cached_reverse_stream_is_lazy() {
    let cached = CachedStore::new_sync(Arc::new(InMemoryStore::new()))
        .await
        .unwrap();
    for key in ["a", "b", "c", "d"] {
        cached
            .set(RecordKey::from(key.as_bytes().to_vec()), Bytes::from(key))
            .await
            .unwrap();
    }
    let mut stream = cached.iter_range_stream_reverse(None, None, 1);
    assert_eq!(stream.next().await.unwrap().unwrap()[0].0.as_ref(), b"d");
    cached
        .set(
            RecordKey::from(b"c".to_vec()),
            Bytes::from_static(b"fresh-c"),
        )
        .await
        .unwrap();
    let next = stream.next().await.unwrap().unwrap();
    assert_eq!(next[0].0.as_ref(), b"c");
    assert_eq!(next[0].1, Bytes::from_static(b"fresh-c"));
}

#[tokio::test]
async fn rv_storage_5e99bede_cached_zero_batch_is_empty() {
    let cached = CachedStore::new_sync(Arc::new(InMemoryStore::new()))
        .await
        .unwrap();
    cached
        .set(
            RecordKey::from(b"present".to_vec()),
            Bytes::from_static(b"v"),
        )
        .await
        .unwrap();
    assert!(cached.iter_stream(0).next().await.is_none());
    assert!(cached
        .scan_prefix_stream(Bytes::from_static(b"p"), 0)
        .next()
        .await
        .is_none());
    assert!(cached
        .iter_range_stream(None, None, 0)
        .next()
        .await
        .is_none());
    assert!(cached
        .iter_range_stream_reverse(None, None, 0)
        .next()
        .await
        .is_none());
}
