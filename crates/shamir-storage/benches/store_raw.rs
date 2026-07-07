//! Raw `Store` trait micro-benchmarks — insert/get/scan per backend.
//!
//! Run: `cargo bench -p shamir-storage --bench store_raw`
//!
//! Measures each backend in isolation, bypassing engine/query layers.
//! Useful for tracking backend regressions and comparing alternatives.
//!
//! # Per-call cost policy
//!
//! The bench-scale-tool harness owns repetition count (calibrated via
//! `--calibrate`), so each registered workload must be a cheap unit
//! (target: ≤10ms per single call). Workloads that vary along a genuine
//! structural axis — `scan`/`prefix_scan` corpus size, or `*_many` batch
//! size — keep their smallest tier as the default and expose larger tiers
//! behind the `BENCH_STORE_RAW_SCALING=1` opt-in env var.
//!
//! Fjall (real on-disk LSM) `scan`/`prefix_scan` are dominated by disk I/O
//! rather than N; reducing N does not proportionally reduce their measured
//! time, so they are kept at the default tier with an inline comment.

use bench_scale_tool::Harness;
use bytes::Bytes;
use shamir_storage::types::Store;
use std::sync::Arc;

const RECORD_SIZE: usize = 256;
const SEED_COUNT: usize = 1000;

/// Default `scan` corpus size (number of records the store is seeded with
/// before a full `iter_stream` sweep). Small enough that in-memory backends
/// stay well under 1ms per call.
const SCAN_N_DEFAULT: usize = 256;

/// Default `prefix_scan` corpus size. Half a thousand shared-prefix records
/// is enough to exercise the range-seek path cheaply.
const PREFIX_SCAN_N_DEFAULT: usize = 512;

/// Default batch size for `set_many` / `get_many` / `remove_many`. These are
/// genuine bulk-throughput axes; the smallest tier is the default.
const MANY_N_DEFAULT: usize = 10;

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

/// Opt-in scaling ladder: when `BENCH_STORE_RAW_SCALING` is set to a truthy
/// value (`1`/`true`/`yes`/`on`), the structural-axis workloads
/// (`scan`, `prefix_scan`, `set_many`, `get_many`, `remove_many`) run their
/// full N ladder instead of just the smallest default tier.
fn scaling_enabled() -> bool {
    std::env::var("BENCH_STORE_RAW_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Returns the N ladder for `scan` (corpus size being swept).
fn scan_ns() -> &'static [usize] {
    static FULL: &[usize] = &[SCAN_N_DEFAULT, 1024, 8192];
    if scaling_enabled() {
        FULL
    } else {
        &[SCAN_N_DEFAULT]
    }
}

/// Returns the N ladder for `prefix_scan` (shared-prefix corpus size).
fn prefix_scan_ns() -> &'static [usize] {
    static FULL: &[usize] = &[PREFIX_SCAN_N_DEFAULT, 5000, 50_000];
    if scaling_enabled() {
        FULL
    } else {
        &[PREFIX_SCAN_N_DEFAULT]
    }
}

/// Returns the N ladder for `*_many` bulk operations (batch size).
fn many_ns() -> &'static [usize] {
    static FULL: &[usize] = &[MANY_N_DEFAULT, 100, 1000];
    if scaling_enabled() {
        FULL
    } else {
        &[MANY_N_DEFAULT]
    }
}

async fn seed(store: &Arc<dyn Store>, n: usize) -> Vec<shamir_storage::types::RecordKey> {
    let mut keys = Vec::with_capacity(n);
    for i in 0..n {
        keys.push(store.insert(make_value(i)).await.unwrap());
    }
    keys
}

fn bench_insert(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    // Single-key insert of a fixed-size record — no N axis; the harness's
    // calibrated repetition count already covers "do this cheap op N times".
    h.bench_async(&format!("{name}/insert"), move || {
        let s = Arc::clone(&store);
        async move {
            s.insert(make_value(0)).await.unwrap();
        }
    });
}

fn bench_get(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    // Single-key get — no N axis (see `bench_insert` rationale).
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

/// `scan` — full `iter_stream` sweep over a seeded corpus. Scan cost scales
/// with corpus size (a genuine structural axis), so the N ladder is exposed
/// via `BENCH_STORE_RAW_SCALING`.
///
/// Note: fjall `scan` is dominated by on-disk I/O (LSM iterator open + page
/// reads) rather than per-record iteration, so it stays expensive even at the
/// smallest default tier — that is a real I/O cost, not an N-repeat artifact.
fn bench_scan(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    for &n in scan_ns() {
        let store = Arc::clone(&store);
        let rt = rt();
        rt.block_on(seed(&store, n));
        h.bench_async(&format!("{name}/scan/{n}"), move || {
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
}

/// `prefix_scan` — `scan_prefix_stream` end-to-end over a shared-prefix
/// corpus. Prefix-scan cost scales with corpus size (structural axis), so the
/// N ladder is exposed via `BENCH_STORE_RAW_SCALING`.
///
/// The 50_000 tier is the canonical "Op A" before/after metric for the
/// range-seek rewrite (O(N²) → O(log N + M) per batch); reach it with
/// `BENCH_STORE_RAW_SCALING=1`.
///
/// Note: fjall `prefix_scan` is dominated by on-disk I/O rather than N.
fn bench_prefix_scan(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    static PREFIX_BYTES: &[u8] = b"pfxscan_";

    for &n in prefix_scan_ns() {
        let store = Arc::clone(&store);
        let rt = rt();

        // Seed `n` records whose keys share the 8-byte prefix.
        rt.block_on(async {
            for i in 0..n {
                let mut key = Vec::with_capacity(16);
                key.extend_from_slice(PREFIX_BYTES);
                key.extend_from_slice(&(i as u64).to_be_bytes());
                let rk = shamir_storage::types::RecordKey::from(key);
                store.set(rk, make_value(i)).await.unwrap();
            }
        });

        let prefix = Bytes::from_static(PREFIX_BYTES);
        h.bench_async(&format!("{name}/prefix_scan/{n}"), move || {
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
}

/// `set_many` — bulk write of a batch. Bulk throughput vs batch size is a
/// genuine structural axis (a single call moves N records), so the N ladder
/// is exposed via `BENCH_STORE_RAW_SCALING`. The harness's repetition count
/// covers repeating the same batch; the ladder varies the batch itself.
fn bench_set_many(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    for &batch in many_ns() {
        let store = Arc::clone(&store);
        let items: Vec<_> = keys[..batch]
            .iter()
            .enumerate()
            .map(|(i, k)| (k.clone(), make_value(i + SEED_COUNT)))
            .collect();
        h.bench_async(&format!("{name}/set_many/{batch}"), move || {
            let s = Arc::clone(&store);
            let items = items.clone();
            async move {
                s.set_many(items).await.unwrap();
            }
        });
    }
}

/// `get_many` — bulk point-read of a batch (structural axis; see
/// `bench_set_many`).
fn bench_get_many(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    for &batch in many_ns() {
        let store = Arc::clone(&store);
        let probe: Vec<_> = keys[..batch].to_vec();
        h.bench_async(&format!("{name}/get_many/{batch}"), move || {
            let s = Arc::clone(&store);
            let probe = probe.clone();
            async move {
                s.get_many(probe).await.unwrap();
            }
        });
    }
}

/// `remove_many` — bulk remove of a batch (structural axis; see
/// `bench_set_many`). The pre-seeded working set means the second iter onward
/// removes tombstoned keys, but the per-key cost shape (dirty insert + cache
/// insert + notify) matches a hot-path remove on an existing key.
fn bench_remove_many(h: &mut Harness, name: &str, store: Arc<dyn Store>) {
    let rt = rt();
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    for &batch in many_ns() {
        let store = Arc::clone(&store);
        let probe: Vec<_> = keys[..batch].to_vec();
        h.bench_async(&format!("{name}/remove_many/{batch}"), move || {
            let s = Arc::clone(&store);
            let probe = probe.clone();
            async move {
                s.remove_many(probe).await.unwrap();
            }
        });
    }
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

/// Fjall bench — includes the prefix_scan tiers (Op A before/after target at
/// the 50_000 rung, behind `BENCH_STORE_RAW_SCALING=1`).
#[cfg(feature = "fjall")]
fn bench_fjall(h: &mut Harness) {
    let dir = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn Store> = make_fjall_store(dir.path());
    let name = "fjall";
    bench_insert(h, name, Arc::clone(&store));
    bench_get(h, name, Arc::clone(&store));
    bench_scan(h, name, Arc::clone(&store));
    bench_set_many(h, name, Arc::clone(&store));
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
