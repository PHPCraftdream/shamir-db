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

criterion_group!(benches, bench_in_memory);
criterion_main!(benches);
