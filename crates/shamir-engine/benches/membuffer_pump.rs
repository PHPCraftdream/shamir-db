//! MemBuffer dirty-pump latency bench.
//!
//! Measures write tail-latency under different MemBufferConfig
//! pressure regimes:
//!   - Sustained insert with default config (no pressure)
//!   - High-frequency flush_interval forcing the background pump
//!   - Byte-pressure eviction (cap small, force evicts)
//!
//! Captures *single-write latency* (not throughput) — that's the
//! number users see when they wait for `set().await`.

use bytes::Bytes;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
use shamir_storage::types::{RecordKey, Store};
use std::sync::Arc;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn make_store(rt: &tokio::runtime::Runtime, cfg: MemBufferConfig) -> Arc<MemBufferStore> {
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    rt.block_on(async { Arc::new(MemBufferStore::new(inner, cfg)) })
}

fn make_value(i: usize) -> Bytes {
    let mut v = vec![0u8; 256];
    v[..8].copy_from_slice(&(i as u64).to_be_bytes());
    Bytes::from(v)
}

fn bench_pump(c: &mut Criterion) {
    let rt = rt();

    let configs = vec![
        (
            "default",
            MemBufferConfig {
                max_bytes: 16 * 1024 * 1024,
                max_entries: 10_000,
                ttl_ms: None,
                flush_interval_ms: 60_000,
                flush_batch_size: 256,
            },
        ),
        (
            "tight_byte_cap",
            MemBufferConfig {
                max_bytes: 64 * 1024, // 64 KB cap → forces evictions
                max_entries: 10_000,
                ttl_ms: None,
                flush_interval_ms: 60_000,
                flush_batch_size: 256,
            },
        ),
        (
            "frequent_flush",
            MemBufferConfig {
                max_bytes: 16 * 1024 * 1024,
                max_entries: 10_000,
                ttl_ms: None,
                flush_interval_ms: 10, // 10ms background pump
                flush_batch_size: 64,
            },
        ),
    ];

    let mut group = c.benchmark_group("membuffer_pump");
    group.throughput(Throughput::Elements(1));

    for (name, cfg) in configs {
        let store = make_store(&rt, cfg);

        // Seed 1000 records to make the pump have actual work
        rt.block_on(async {
            for i in 0..1000 {
                let _ = store.insert(make_value(i)).await.unwrap();
            }
        });

        // bench: single insert latency under this regime
        group.bench_with_input(BenchmarkId::new("insert_single", name), &name, |b, _| {
            let s = Arc::clone(&store);
            b.to_async(&rt).iter(|| {
                let s = Arc::clone(&s);
                async move {
                    let _ = s.insert(make_value(0)).await.unwrap();
                }
            });
        });

        // bench: single get latency (cache hit path)
        let warm_keys: Vec<RecordKey> = rt.block_on(async {
            let mut keys = Vec::with_capacity(100);
            for i in 0..100 {
                let k = store.insert(make_value(i + 100_000)).await.unwrap();
                keys.push(k);
            }
            keys
        });
        let key = warm_keys[50].clone();

        group.bench_with_input(BenchmarkId::new("get_single", name), &name, |b, _| {
            let s = Arc::clone(&store);
            let k = key.clone();
            b.to_async(&rt).iter(|| {
                let s = Arc::clone(&s);
                let k = k.clone();
                async move {
                    let _ = s.get(k).await.unwrap();
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_pump);
criterion_main!(benches);
