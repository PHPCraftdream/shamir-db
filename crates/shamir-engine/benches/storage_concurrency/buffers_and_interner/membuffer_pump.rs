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
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): each
//! `MemBufferStore` (with its seeded 1000 records + warm keys) is built
//! ONCE per config, outside the timed closure — plan 1 (shared setup).
//! The harness owns the single shared tokio runtime for `bench_async`.

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
use shamir_storage::types::{RecordKey, Store};

async fn make_store(cfg: MemBufferConfig) -> Arc<MemBufferStore> {
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    Arc::new(MemBufferStore::new(inner, cfg))
}

fn make_value(i: usize) -> Bytes {
    let mut v = vec![0u8; 256];
    v[..8].copy_from_slice(&(i as u64).to_be_bytes());
    Bytes::from(v)
}

fn main() {
    let mut h = Harness::new("membuffer_pump", env!("CARGO_MANIFEST_DIR"));

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

    // Build a temporary single-thread runtime purely for the untimed setup
    // phase (seeding fixtures) — the harness's own shared runtime only
    // drives the registered `bench_async` closures.
    let setup_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    for (name, cfg) in configs {
        let store = setup_rt.block_on(make_store(cfg));

        // Seed 1000 records to make the pump have actual work.
        setup_rt.block_on(async {
            for i in 0..1000 {
                let _ = store.insert(make_value(i)).await.unwrap();
            }
        });

        // bench: single insert latency under this regime.
        {
            let s = Arc::clone(&store);
            h.bench_async(&format!("membuffer_pump/insert_single_{name}"), move || {
                let s = Arc::clone(&s);
                async move {
                    let _ = s.insert(make_value(0)).await.unwrap();
                }
            });
        }

        // bench: single get latency (cache hit path).
        let warm_keys: Vec<RecordKey> = setup_rt.block_on(async {
            let mut keys = Vec::with_capacity(100);
            for i in 0..100 {
                let k = store.insert(make_value(i + 100_000)).await.unwrap();
                keys.push(k);
            }
            keys
        });
        let key = warm_keys[50].clone();

        {
            let s = Arc::clone(&store);
            let k = key.clone();
            h.bench_async(&format!("membuffer_pump/get_single_{name}"), move || {
                let s = Arc::clone(&s);
                let k = k.clone();
                async move {
                    let _ = black_box(s.get(k).await.unwrap());
                }
            });
        }
    }

    h.run();
}
