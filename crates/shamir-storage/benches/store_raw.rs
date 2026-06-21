//! Raw `Store` trait micro-benchmarks — insert/get/scan per backend.
//!
//! Run: `cargo bench -p shamir-storage --bench store_raw`
//!
//! Measures each backend in isolation, bypassing engine/query layers.
//! Useful for tracking backend regressions and comparing alternatives.

use bytes::Bytes;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_storage::types::Store;
use std::sync::Arc;

const RECORD_SIZE: usize = 256;
const SEED_COUNT: usize = 1000;

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

macro_rules! disk_backend {
    ($feat:literal, $mod_name:ident, $bench_fn:ident, $name:expr, $make:expr) => {
        #[cfg(feature = $feat)]
        fn $bench_fn(c: &mut Criterion) {
            let dir = tempfile::TempDir::new().unwrap();
            let store: Arc<dyn Store> = $make(dir.path());
            bench_insert(c, $name, Arc::clone(&store));
            bench_get(c, $name, Arc::clone(&store));
            bench_scan(c, $name, Arc::clone(&store));
            bench_set_many(c, $name, store);
        }
    };
}

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

disk_backend!("sled", storage_sled, bench_sled, "sled", make_sled_store);
disk_backend!(
    "fjall",
    storage_fjall,
    bench_fjall,
    "fjall",
    make_fjall_store
);

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
