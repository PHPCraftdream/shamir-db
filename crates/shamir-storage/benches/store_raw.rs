//! Raw `Store` trait micro-benchmarks — insert/get/scan per backend.
//!
//! Run: `cargo bench -p shamir-storage --bench store_raw`
//!
//! Measures each backend in isolation, bypassing engine/query layers.
//! Useful for tracking backend regressions and comparing alternatives.

use bytes::Bytes;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_bench_utils::tune_tiered;
use shamir_storage::types::Store;
use std::sync::Arc;

const RECORD_SIZE: usize = 256;
const SEED_COUNT: usize = 1000;

/// Prefix scan dataset: 50k records that share a common 8-byte prefix.
/// Used by `bench_prefix_scan` to measure the cost of `scan_prefix_stream`.
const PREFIX_SCAN_N: usize = 50_000;

fn make_value(i: usize) -> Bytes {
    let mut v = vec![0u8; RECORD_SIZE];
    v[..8].copy_from_slice(&(i as u64).to_be_bytes());
    Bytes::from(v)
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

async fn seed(store: &Arc<dyn Store>, n: usize) -> Vec<shamir_storage::types::RecordKey> {
    let mut keys = Vec::with_capacity(n);
    for i in 0..n {
        keys.push(store.insert(make_value(i)).await.unwrap());
    }
    keys
}

fn bench_insert(c: &mut Criterion, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    let mut group = c.benchmark_group(format!("{name}/insert"));
    group.throughput(Throughput::Elements(1));
    group.bench_function("single", |b| {
        let s = Arc::clone(&store);
        b.to_async(&rt).iter(|| async {
            s.insert(make_value(0)).await.unwrap();
        });
    });
    group.finish();
}

fn bench_get(c: &mut Criterion, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    let mut group = c.benchmark_group(format!("{name}/get"));
    group.throughput(Throughput::Elements(1));
    let key = keys[SEED_COUNT / 2].clone();
    group.bench_function("single", |b| {
        let s = Arc::clone(&store);
        let k = key.clone();
        b.to_async(&rt).iter(|| {
            let s = Arc::clone(&s);
            let k = k.clone();
            async move {
                s.get(k).await.unwrap();
            }
        });
    });
    group.finish();
}

fn bench_scan(c: &mut Criterion, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    rt.block_on(seed(&store, SEED_COUNT));
    let mut group = c.benchmark_group(format!("{name}/scan"));
    group.throughput(Throughput::Elements(SEED_COUNT as u64));
    group.bench_function(BenchmarkId::new("iter_stream", SEED_COUNT), |b| {
        let s = Arc::clone(&store);
        b.to_async(&rt).iter(|| {
            let s = Arc::clone(&s);
            async move {
                use futures::StreamExt;
                let mut stream = s.iter_stream(256);
                let mut count = 0u64;
                while let Some(batch) = stream.next().await {
                    count += batch.unwrap().len() as u64;
                }
                count
            }
        });
    });
    group.finish();
}

/// Prefix-scan benchmark — measures `scan_prefix_stream` end-to-end throughput
/// on a 50k-record store where ALL records share an 8-byte prefix.
///
/// This is the worst-case shape for the old full-iter+linear-seek implementation
/// (O(N²) re-scan on every batch) and the best demonstration of the
/// range-seek rewrite (O(log N + M) per batch).
///
/// `prefix_scan/<backend>/50000` is the canonical Op A before/after metric.
fn bench_prefix_scan(c: &mut Criterion, name: &str, store: Arc<dyn Store>) {
    use futures::StreamExt;

    let rt = rt();

    // Shared 8-byte prefix — every record uses it.
    static PREFIX_BYTES: &[u8] = b"pfxscan_";
    let prefix = Bytes::from_static(PREFIX_BYTES);

    // Seed PREFIX_SCAN_N records whose keys start with the prefix.
    // We set explicit keys via `store.set(key, value)` so we control the prefix.
    rt.block_on(async {
        // Build keys: prefix (8 bytes) + 8-byte big-endian index.
        for i in 0..PREFIX_SCAN_N {
            let mut key = Vec::with_capacity(16);
            key.extend_from_slice(PREFIX_BYTES);
            key.extend_from_slice(&(i as u64).to_be_bytes());
            let rk = shamir_storage::types::RecordKey::from(key);
            store.set(rk, make_value(i)).await.unwrap();
        }
    });

    let mut group = c.benchmark_group(format!("{name}/prefix_scan"));
    group.throughput(Throughput::Elements(PREFIX_SCAN_N as u64));
    tune_tiered(&mut group, 20, 5, 3, 120);

    group.bench_function(BenchmarkId::new("scan_prefix_stream", PREFIX_SCAN_N), |b| {
        let s = Arc::clone(&store);
        let pfx = prefix.clone();
        b.to_async(&rt).iter(|| {
            let s = Arc::clone(&s);
            let pfx = pfx.clone();
            async move {
                let mut stream = s.scan_prefix_stream(pfx, 256);
                let mut count = 0u64;
                while let Some(batch) = stream.next().await {
                    count += batch.unwrap().len() as u64;
                }
                count
            }
        });
    });
    group.finish();
}

fn bench_set_many(c: &mut Criterion, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    let mut group = c.benchmark_group(format!("{name}/set_many"));
    let batch_size = 100;
    group.throughput(Throughput::Elements(batch_size as u64));
    let items: Vec<_> = keys[..batch_size]
        .iter()
        .enumerate()
        .map(|(i, k)| (k.clone(), make_value(i + SEED_COUNT)))
        .collect();
    group.bench_function(BenchmarkId::new("batch", batch_size), |b| {
        let s = Arc::clone(&store);
        let items = items.clone();
        b.to_async(&rt).iter(|| {
            let s = Arc::clone(&s);
            let items = items.clone();
            async move {
                s.set_many(items).await.unwrap();
            }
        });
    });
    group.finish();
}

fn bench_get_many(c: &mut Criterion, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    let mut group = c.benchmark_group(format!("{name}/get_many"));
    let batch_size = 100;
    group.throughput(Throughput::Elements(batch_size as u64));
    let probe: Vec<_> = keys[..batch_size].to_vec();
    group.bench_function(BenchmarkId::new("batch", batch_size), |b| {
        let s = Arc::clone(&store);
        let probe = probe.clone();
        b.to_async(&rt).iter(|| {
            let s = Arc::clone(&s);
            let probe = probe.clone();
            async move {
                s.get_many(probe).await.unwrap();
            }
        });
    });
    group.finish();
}

fn bench_remove_many(c: &mut Criterion, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    // Pre-seed a working set; the bench removes the same set of keys
    // every iter — the second iter onward they're tombstoned, but the
    // per-key cost shape (dirty insert + cache insert + notify) is the
    // same as a hot-path remove on an existing key.
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    let mut group = c.benchmark_group(format!("{name}/remove_many"));
    let batch_size = 100;
    group.throughput(Throughput::Elements(batch_size as u64));
    let probe: Vec<_> = keys[..batch_size].to_vec();
    group.bench_function(BenchmarkId::new("batch", batch_size), |b| {
        let s = Arc::clone(&store);
        let probe = probe.clone();
        b.to_async(&rt).iter(|| {
            let s = Arc::clone(&s);
            let probe = probe.clone();
            async move {
                s.remove_many(probe).await.unwrap();
            }
        });
    });
    group.finish();
}

// ────────────────────────────────────────────────────────────────────
// In-memory backend (always available)
// ────────────────────────────────────────────────────────────────────

fn in_memory_store() -> Arc<dyn Store> {
    Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new())
}

fn bench_in_memory(c: &mut Criterion) {
    let name = "in_memory";
    bench_insert(c, name, in_memory_store());
    bench_get(c, name, in_memory_store());
    bench_scan(c, name, in_memory_store());
    bench_set_many(c, name, in_memory_store());
}

// ────────────────────────────────────────────────────────────────────
// Disk backends (feature-gated)
// ────────────────────────────────────────────────────────────────────

#[cfg(feature = "sled")]
fn make_sled_store(dir: &std::path::Path) -> Arc<dyn Store> {
    use shamir_storage::types::Repo;
    let repo = shamir_storage::storage_sled::SledRepo::new(dir.join("sled")).unwrap();
    let rt = rt();
    rt.block_on(repo.store_get("bench")).unwrap()
}

#[cfg(feature = "fjall")]
fn make_fjall_store(dir: &std::path::Path) -> Arc<dyn Store> {
    use shamir_storage::types::Repo;
    let repo = shamir_storage::storage_fjall::FjallRepo::new(dir.join("fjall")).unwrap();
    let rt = rt();
    rt.block_on(repo.store_get("bench")).unwrap()
}

/// Sled bench — includes the prefix_scan 50k cell (Op A.2 before/after target).
#[cfg(feature = "sled")]
fn bench_sled(c: &mut Criterion) {
    let dir = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn Store> = make_sled_store(dir.path());
    let name = "sled";
    bench_insert(c, name, Arc::clone(&store));
    bench_get(c, name, Arc::clone(&store));
    bench_scan(c, name, Arc::clone(&store));
    bench_set_many(c, name, Arc::clone(&store));
    // Op A.2 benchmark: prefix_scan on 50k shared-prefix records.
    bench_prefix_scan(c, name, store);
}

/// Fjall bench — includes the prefix_scan 50k cell (Op A before/after target).
#[cfg(feature = "fjall")]
fn bench_fjall(c: &mut Criterion) {
    let dir = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn Store> = make_fjall_store(dir.path());
    let name = "fjall";
    bench_insert(c, name, Arc::clone(&store));
    bench_get(c, name, Arc::clone(&store));
    bench_scan(c, name, Arc::clone(&store));
    bench_set_many(c, name, Arc::clone(&store));
    // Op A benchmark: prefix_scan on 50k shared-prefix records.
    bench_prefix_scan(c, name, store);
}

fn cached_in_memory_store() -> Arc<dyn Store> {
    let inner = in_memory_store();
    let rt = rt();
    Arc::new(
        rt.block_on(shamir_storage::storage_cached::CachedStore::new_sync(inner))
            .unwrap(),
    )
}

fn bench_cached_in_memory(c: &mut Criterion) {
    let name = "cached_in_memory";
    bench_insert(c, name, cached_in_memory_store());
    bench_get(c, name, cached_in_memory_store());
    bench_scan(c, name, cached_in_memory_store());
    bench_set_many(c, name, cached_in_memory_store());
}

fn membuffer_in_memory_store() -> Arc<dyn Store> {
    let inner = in_memory_store();
    let cfg = shamir_storage::storage_membuffer::MemBufferConfig {
        max_bytes: 64 * 1024 * 1024,
        max_entries: 1_000_000,
        ttl_ms: None,
        flush_interval_ms: 60_000,
        flush_batch_size: 256,
    };
    let rt = rt();
    rt.block_on(async {
        Arc::new(shamir_storage::storage_membuffer::MemBufferStore::new(
            inner, cfg,
        )) as Arc<dyn Store>
    })
}

fn bench_membuffer_in_memory(c: &mut Criterion) {
    let name = "membuffer_in_memory";
    bench_set_many(c, name, membuffer_in_memory_store());
    bench_get_many(c, name, membuffer_in_memory_store());
    bench_remove_many(c, name, membuffer_in_memory_store());
}

fn bench_all_backends(c: &mut Criterion) {
    bench_in_memory(c);
    bench_cached_in_memory(c);
    bench_membuffer_in_memory(c);
    #[cfg(feature = "sled")]
    bench_sled(c);
    #[cfg(feature = "fjall")]
    bench_fjall(c);
}

criterion_group!(benches, bench_all_backends);
criterion_main!(benches);
