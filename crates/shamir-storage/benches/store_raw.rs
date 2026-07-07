//! Raw `Store` trait micro-benchmarks — insert/get/scan per backend.
//!
//! Run: `cargo bench -p shamir-storage --bench store_raw`
//!
//! Measures each backend in isolation, bypassing engine/query layers.
//! Useful for tracking backend regressions and comparing alternatives.

use bench_scale_tool::Harness;
use bytes::Bytes;
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

fn bench_insert(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    h.bench_async(&format!("{name}/insert"), move || {
        let s = Arc::clone(&store);
        async move {
            s.insert(make_value(0)).await.unwrap();
        }
    });
}

fn bench_get(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    let key = keys[SEED_COUNT / 2].clone();
    h.bench_async(&format!("{name}/get"), move || {
        let s = Arc::clone(&store);
        let k = key.clone();
        async move {
            s.get(k).await.unwrap();
        }
    });
}

fn bench_scan(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    rt.block_on(seed(&store, SEED_COUNT));
    h.bench_async(&format!("{name}/scan"), move || {
        let s = Arc::clone(&store);
        async move {
            use futures::StreamExt;
            let mut stream = s.iter_stream(256);
            let mut count = 0u64;
            while let Some(batch) = stream.next().await {
                count += batch.unwrap().len() as u64;
            }
            std::hint::black_box(count);
        }
    });
}

/// Prefix-scan benchmark — measures `scan_prefix_stream` end-to-end throughput
/// on a 50k-record store where ALL records share an 8-byte prefix.
///
/// This is the worst-case shape for the old full-iter+linear-seek implementation
/// (O(N²) re-scan on every batch) and the best demonstration of the
/// range-seek rewrite (O(log N + M) per batch).
///
/// `prefix_scan/<backend>/50000` is the canonical Op A before/after metric.
fn bench_prefix_scan(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
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

    h.bench_async(&format!("{name}/prefix_scan"), move || {
        use futures::StreamExt;
        let s = Arc::clone(&store);
        let pfx = prefix.clone();
        async move {
            let mut stream = s.scan_prefix_stream(pfx, 256);
            let mut count = 0u64;
            while let Some(batch) = stream.next().await {
                count += batch.unwrap().len() as u64;
            }
            std::hint::black_box(count);
        }
    });
}

fn bench_set_many(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    let batch_size = 100;
    let items: Vec<_> = keys[..batch_size]
        .iter()
        .enumerate()
        .map(|(i, k)| (k.clone(), make_value(i + SEED_COUNT)))
        .collect();
    h.bench_async(&format!("{name}/set_many"), move || {
        let s = Arc::clone(&store);
        let items = items.clone();
        async move {
            s.set_many(items).await.unwrap();
        }
    });
}

fn bench_get_many(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    let batch_size = 100;
    let probe: Vec<_> = keys[..batch_size].to_vec();
    h.bench_async(&format!("{name}/get_many"), move || {
        let s = Arc::clone(&store);
        let probe = probe.clone();
        async move {
            s.get_many(probe).await.unwrap();
        }
    });
}

fn bench_remove_many(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    // Pre-seed a working set; the bench removes the same set of keys
    // every iter — the second iter onward they're tombstoned, but the
    // per-key cost shape (dirty insert + cache insert + notify) is the
    // same as a hot-path remove on an existing key.
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    let batch_size = 100;
    let probe: Vec<_> = keys[..batch_size].to_vec();
    h.bench_async(&format!("{name}/remove_many"), move || {
        let s = Arc::clone(&store);
        let probe = probe.clone();
        async move {
            s.remove_many(probe).await.unwrap();
        }
    });
}

// ────────────────────────────────────────────────────────────────────
// In-memory backend (always available)
// ────────────────────────────────────────────────────────────────────

fn in_memory_store() -> Arc<dyn Store> {
    Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new())
}

fn bench_in_memory(h: &mut Harness) {
    let name = "in_memory";
    bench_insert(h, name, in_memory_store());
    bench_get(h, name, in_memory_store());
    bench_scan(h, name, in_memory_store());
    bench_set_many(h, name, in_memory_store());
}

// ────────────────────────────────────────────────────────────────────
// Disk backends (feature-gated)
// ────────────────────────────────────────────────────────────────────

#[cfg(feature = "fjall")]
fn make_fjall_store(dir: &std::path::Path) -> Arc<dyn Store> {
    use shamir_storage::types::Repo;
    let repo = shamir_storage::storage_fjall::FjallRepo::new(dir.join("fjall")).unwrap();
    let rt = rt();
    rt.block_on(repo.store_get("bench")).unwrap()
}

/// Fjall bench — includes the prefix_scan 50k cell (Op A before/after target).
#[cfg(feature = "fjall")]
fn bench_fjall(h: &mut Harness) {
    let dir = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn Store> = make_fjall_store(dir.path());
    let name = "fjall";
    bench_insert(h, name, Arc::clone(&store));
    bench_get(h, name, Arc::clone(&store));
    bench_scan(h, name, Arc::clone(&store));
    bench_set_many(h, name, Arc::clone(&store));
    // Op A benchmark: prefix_scan on 50k shared-prefix records.
    bench_prefix_scan(h, name, store);
    // Keep the temp dir alive for the lifetime of the registered closures.
    std::mem::forget(dir);
}

fn cached_in_memory_store() -> Arc<dyn Store> {
    let inner = in_memory_store();
    let rt = rt();
    Arc::new(
        rt.block_on(shamir_storage::storage_cached::CachedStore::new_sync(inner))
            .unwrap(),
    )
}

fn bench_cached_in_memory(h: &mut Harness) {
    let name = "cached_in_memory";
    bench_insert(h, name, cached_in_memory_store());
    bench_get(h, name, cached_in_memory_store());
    bench_scan(h, name, cached_in_memory_store());
    bench_set_many(h, name, cached_in_memory_store());
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

fn bench_membuffer_in_memory(h: &mut Harness) {
    let name = "membuffer_in_memory";
    bench_set_many(h, name, membuffer_in_memory_store());
    bench_get_many(h, name, membuffer_in_memory_store());
    bench_remove_many(h, name, membuffer_in_memory_store());
}

fn main() {
    let mut h = Harness::new("store_raw", env!("CARGO_MANIFEST_DIR"));
    bench_in_memory(&mut h);
    bench_cached_in_memory(&mut h);
    bench_membuffer_in_memory(&mut h);
    #[cfg(feature = "fjall")]
    bench_fjall(&mut h);
    h.run();
}
