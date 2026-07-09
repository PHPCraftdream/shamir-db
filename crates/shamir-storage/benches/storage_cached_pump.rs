//! Dedicated `CachedStore` pump — isolates the two cache-layer findings
//! from audit `2026-07-06-perf-radical-o-notation`:
//!
//!   §1.4 — `transact` with `Set` must POPULATE the cache (not
//!          invalidate), so read-after-write hits RAM instead of the
//!          (disk) backend. The previous code removed every touched
//!          key post-commit → every read-after-write systematically
//!          missed the cache → ×10-100 per such read.
//!   §1.3 — `iter_stream` / `scan_prefix_stream` must yield batches
//!          INCREMENTALLY (cursor/resume by last key), not eagerly
//!          collect the entire corpus before the first yield. A
//!          `LIMIT`-style consumer still paid O(N) clones + alloc.
//!
//! Run: `cargo bench -p shamir-storage --bench storage_cached_pump`
//!
//! # Workloads
//!
//! §1.4 read-after-write (the audit's core claim): a `transact([Set])`
//! commits a key, then a `get` reads it back. The fix makes that `get`
//! a cache HIT (RAM); the old invalidate-path made it a cache MISS
//! (backend round-trip). We isolate the READ cost in two paired
//! workloads so the ratio is a clean cache-hit vs cache-miss signal:
//!
//! - `get_cache_hit` — `get` on a key present in the cache (the
//!   after-fix read-after-write state). Measures pure cache-lookup cost.
//! - `get_cache_miss` — `get` on a key NOT in the cache, falling
//!   through to the inner store + caching it. This is the per-read
//!   cost the OLD invalidate-path paid on EVERY read-after-write.
//!   The ratio `get_cache_miss / get_cache_hit` is the §1.4 speedup.
//!
//! §1.3 stream eagerness: open a stream on a LARGE cache (10k entries)
//! and compare consuming ONLY the first batch vs draining ALL batches.
//! With the fix the first-batch consumer pays O(batch_size); the old
//! eager-collect paid O(N) regardless of how little was consumed.
//!
//! - `iter_first_batch` / `iter_full_drain` — for `iter_stream`.
//! - `scan_prefix_first_batch` / `scan_prefix_full_drain` — for
//!   `scan_prefix_stream`.
//!
//! The ratio `*_full_drain / *_first_batch` is the §1.3 speedup for
//! an early-exiting consumer.
//!
//! # Honest-reporting note
//!
//! The OLD (pre-fix) code can no longer be re-measured without
//! reverting the source, so there is no literal "before" column from a
//! fresh run. Instead, each workload is paired with its natural
//! worst-case counterpart (`get_cache_miss` for the cache-hit case;
//! `*_full_drain` for the early-exit case) — the ratio between the
//! pair IS the speedup the fix delivers. This mirrors task #486's
//! precedent of reporting honest, paired measurements.

use bench_scale_tool::Harness;
use bytes::Bytes;
use shamir_storage::storage_cached::CachedStore;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};
use std::sync::Arc;

/// Record size — large enough that a clone/memcpy is a measurable
/// fraction of per-op cost (so the eager-collect O(N) cost is
/// visible), small enough to stay in cache.
const RECORD_SIZE: usize = 256;

/// Corpus size for the stream workloads — large enough that an
/// eager O(N) collect dominates the per-batch yield cost.
const STREAM_CORPUS: usize = 10_000;

fn make_value(i: usize) -> Bytes {
    let mut v = vec![0u8; RECORD_SIZE];
    v[..8].copy_from_slice(&(i as u64).to_be_bytes());
    for (j, b) in v[8..].iter_mut().enumerate() {
        *b = ((i as u64).wrapping_add(j as u64)) as u8;
    }
    Bytes::from(v)
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Build a fresh `CachedStore` over an `InMemoryStore` inner.
/// `InMemoryStore` is itself an in-RAM B+ tree, so this isolates the
/// `CachedStore` logic (cache hit vs miss, stream eagerness) from disk
/// I/O — the §1.4 / §1.3 findings are about the cache layer's
/// accounting, not the backend's latency.
fn make_cached_store() -> Arc<CachedStore> {
    let inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let cached = rt().block_on(CachedStore::new_sync(inner)).unwrap();
    Arc::new(cached)
}

/// `get` on a key that IS in the cache — the after-fix read-after-write
/// state. The §1.4 fix populates the cache on `transact(Set)`, so the
/// immediate `get` is a pure cache lookup (no backend round-trip). We
/// seed the key via `transact(Set)` (the exact write path the audit
/// describes) and then repeatedly `get` it to measure pure cache-read
/// cost — the cost a read-after-write pays AFTER the fix.
fn bench_get_cache_hit(h: &mut Harness, store: Arc<CachedStore>) {
    let store_dyn: Arc<dyn Store> = store.clone();
    // Seed one key via transact(Set) — the exact write path the audit
    // flags. After the commit the key is in the cache (the fix).
    let key = rt().block_on(async {
        let k = RecordKey::from_static(b"raw-hit-key");
        store_dyn
            .transact(vec![KvOp::Set(k.clone(), make_value(0))])
            .await
            .unwrap();
        k
    });
    h.bench_async("get_cache_hit", move || {
        let s = Arc::clone(&store_dyn);
        let k = key.clone();
        async move {
            let got = s.get(k).await.unwrap();
            std::hint::black_box(got);
        }
    });
}

/// `get` on a key NOT in the cache — falls through to the inner
/// `InMemoryStore` + caches the result. This is the per-read cost the
/// OLD invalidate-path paid on EVERY read-after-write (the `transact`
/// removed the key, so the `get` always missed). The ratio
/// `get_cache_miss / get_cache_hit` quantifies the §1.4 speedup.
///
/// We rotate keys so the cache never warms for long (each key is read
/// once, then the cursor moves on — mirroring a scan-like
/// read-after-write pattern).
fn bench_get_cache_miss(h: &mut Harness, store: Arc<CachedStore>) {
    // Seed N keys into the INNER store directly, bypassing the cache,
    // so the cache is cold on every key.
    let inner = store.inner().clone();
    let n = 2_000usize;
    let keys: Vec<RecordKey> = rt().block_on(async {
        let mut ks = Vec::with_capacity(n);
        for i in 0..n {
            ks.push(inner.insert(make_value(i)).await.unwrap());
        }
        ks
    });

    let store_dyn: Arc<dyn Store> = store.clone();
    let mut idx: usize = 0;
    h.bench_async("get_cache_miss", move || {
        let s = Arc::clone(&store_dyn);
        let k = keys[idx % keys.len()].clone();
        idx = idx.wrapping_add(1);
        let _ = idx; // silence unused-assignment on the final iteration
        async move {
            // Cache miss → loads from inner, then caches. We pay the
            // inner round-trip + the cache-insert on every call (the
            // key rotates so the cache never warms for long).
            let got = s.get(k).await.unwrap();
            std::hint::black_box(got);
        }
    });
}

/// `iter_stream` on a LARGE cache (10k entries), consume ONLY the
/// first batch, then drop the stream. With the §1.3 fix this pays
/// O(batch_size) clones; the old eager-collect paid O(N) before the
/// first yield regardless of how little the consumer drained.
fn bench_iter_first_batch(h: &mut Harness, store: Arc<CachedStore>) {
    seed_corpus(&store, STREAM_CORPUS);
    let store_dyn: Arc<dyn Store> = store.clone();
    h.bench_async("iter_first_batch", move || {
        let s = Arc::clone(&store_dyn);
        async move {
            use futures::StreamExt;
            let mut stream = s.iter_stream(64);
            let first = stream.next().await.expect("first batch").unwrap();
            std::hint::black_box(first);
            // Drop the stream here — only the first batch was materialized.
            drop(stream);
        }
    });
}

/// `iter_stream` on the same LARGE cache, DRAIN every batch. This is
/// the "full consumption" cost; paired with `iter_first_batch` it
/// shows the old eager path paid the full cost even for a 1-batch
/// consumer (the ratio `iter_full_drain / iter_first_batch` is the
/// §1.3 speedup for an early-exiting consumer).
fn bench_iter_full_drain(h: &mut Harness, store: Arc<CachedStore>) {
    seed_corpus(&store, STREAM_CORPUS);
    let store_dyn: Arc<dyn Store> = store.clone();
    h.bench_async("iter_full_drain", move || {
        let s = Arc::clone(&store_dyn);
        async move {
            use futures::StreamExt;
            let mut stream = s.iter_stream(64);
            let mut count = 0u64;
            while let Some(batch) = stream.next().await {
                count += batch.unwrap().len() as u64;
            }
            std::hint::black_box(count);
        }
    });
}

/// `scan_prefix_stream` first-batch-only — same early-exit shape as
/// `iter_first_batch` but for the prefix-scan path (§1.3).
fn bench_scan_prefix_first_batch(h: &mut Harness, store: Arc<CachedStore>) {
    seed_prefix_corpus(&store, STREAM_CORPUS, b"pfx_");
    let prefix = Bytes::from_static(b"pfx_");
    let store_dyn: Arc<dyn Store> = store.clone();
    h.bench_async("scan_prefix_first_batch", move || {
        let s = Arc::clone(&store_dyn);
        let pfx = prefix.clone();
        async move {
            use futures::StreamExt;
            let mut stream = s.scan_prefix_stream(pfx, 64);
            let first = stream.next().await.expect("first batch").unwrap();
            std::hint::black_box(first);
            drop(stream);
        }
    });
}

/// `scan_prefix_stream` full drain — counterpart to
/// `scan_prefix_first_batch`.
fn bench_scan_prefix_full_drain(h: &mut Harness, store: Arc<CachedStore>) {
    seed_prefix_corpus(&store, STREAM_CORPUS, b"pfx_");
    let prefix = Bytes::from_static(b"pfx_");
    let store_dyn: Arc<dyn Store> = store.clone();
    h.bench_async("scan_prefix_full_drain", move || {
        let s = Arc::clone(&store_dyn);
        let pfx = prefix.clone();
        async move {
            use futures::StreamExt;
            let mut stream = s.scan_prefix_stream(pfx, 64);
            let mut count = 0u64;
            while let Some(batch) = stream.next().await {
                count += batch.unwrap().len() as u64;
            }
            std::hint::black_box(count);
        }
    });
}

/// Seed `n` sequential-key records into the cache via `set` (so they
/// land in the cache directly, no inner round-trip needed for the
/// stream benches).
fn seed_corpus(store: &Arc<CachedStore>, n: usize) {
    rt().block_on(async {
        let store_dyn: Arc<dyn Store> = store.clone() as Arc<dyn Store>;
        // Use fixed-width numeric keys so TreeIndex lex order == numeric
        // order (stable, predictable for the cursor resume).
        for i in 0..n {
            let key = RecordKey::from(format!("k{:08}", i).into_bytes());
            store_dyn.set(key, make_value(i)).await.unwrap();
        }
    });
}

/// Seed `n` records under a shared `prefix` for the prefix-scan benches.
fn seed_prefix_corpus(store: &Arc<CachedStore>, n: usize, prefix: &[u8]) {
    rt().block_on(async {
        let store_dyn: Arc<dyn Store> = store.clone() as Arc<dyn Store>;
        for i in 0..n {
            let mut key = Vec::with_capacity(prefix.len() + 8);
            key.extend_from_slice(prefix);
            key.extend_from_slice(&format!("{:08}", i).into_bytes());
            store_dyn
                .set(RecordKey::from(key), make_value(i))
                .await
                .unwrap();
        }
    });
}

fn main() {
    let mut h = Harness::new("storage_cached_pump", env!("CARGO_MANIFEST_DIR"));

    // §1.4 — read-after-write: cache hit vs cache miss. Each gets its
    // own fresh store.
    bench_get_cache_hit(&mut h, make_cached_store());
    bench_get_cache_miss(&mut h, make_cached_store());

    // §1.3 — stream eagerness: first-batch-only vs full drain, for
    // both iter_stream and scan_prefix_stream. Each pair shares a
    // fresh store seeded with STREAM_CORPUS entries.
    bench_iter_first_batch(&mut h, make_cached_store());
    bench_iter_full_drain(&mut h, make_cached_store());
    bench_scan_prefix_first_batch(&mut h, make_cached_store());
    bench_scan_prefix_full_drain(&mut h, make_cached_store());

    h.run();
}
