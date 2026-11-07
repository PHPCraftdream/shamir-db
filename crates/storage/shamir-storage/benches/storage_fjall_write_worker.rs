//! Cold-cache, high-fan-out `FjallStore` bench for task #536 (the write-worker
//! redesign).
//!
//! Run: `cargo bench -p shamir-storage --bench storage_fjall_write_worker`
//!
//! # Why this bench exists (and why the small `storage_fjall_pump` is not
//! enough)
//!
//! The reverted task #524 prototype (see
//! `docs/dev-artifacts/design/fjall-worker-loop-524-findings.md`) was benched against a
//! ~1 MiB, fully memtable-resident dataset with no write fan-out, so its
//! numbers could not expose the two things that actually matter for a
//! write-serialization redesign:
//!
//!   1. **Concurrent WRITE contention** — many callers hammering `insert`/`set`
//!      at once. The whole thesis of routing writes through one worker is
//!      "fjall already serializes writes on its per-Database journal mutex, so
//!      one worker loses no parallelism and saves the per-op `spawn_blocking`
//!      hop." That thesis held for `insert`/`transact` (no embedded read) but
//!      measurably did NOT hold for `set`/`remove` (embedded `contains_key`
//!      read loses spawn_blocking-pool parallelism when serialized onto one
//!      worker) — see `storage_fjall.rs`'s write-worker module comment for
//!      the numbers this bench produced and why `set`/`remove` stayed on
//!      `spawn_blocking` as a result. That distinction is only observable
//!      under real fan-out.
//!   2. **Cold-cache READS** — fjall's default block cache is 32 MiB. A dataset
//!      several times larger forces genuine on-disk reads (bloom + index +
//!      data block), the reads most exposed to any accidental serialization.
//!      This bench MUST show reads did NOT regress (they stay on the untouched
//!      `spawn_blocking` path).
//!
//! # Dataset sizing
//!
//! `SEED_COUNT * RECORD_SIZE` ≈ 256 MiB — **8× fjall's 32 MiB default block
//! cache** — and we `flush()` after seeding so segments are on disk. Scattered
//! point-reads across the full keyset therefore miss the cache and do real I/O.
//!
//! # Workloads
//!
//! - `write_concurrent_insert` — each timed iteration fans out `WRITE_FANOUT`
//!   concurrent fresh-key `insert` calls and awaits them all. `insert` IS
//!   routed through the write worker (no embedded read) — measures the
//!   expected win: dispatch amortization + one worker vs N per-op
//!   `spawn_blocking` hops.
//! - `write_concurrent_set` — same shape, but `set` (each call overwrites an
//!   existing key, paying an embedded `contains_key` probe + insert). `set`
//!   is deliberately NOT routed through the write worker (measured here to
//!   regress ~1.45× when it was) — this workload is the CONTROL / regression
//!   guard proving `set` stays at its original `spawn_blocking` performance,
//!   not a workload the worker targets.
//! - `read_cold_scattered` — each timed iteration fans out `READ_FANOUT`
//!   concurrent point-reads at pseudo-random keys across the 256 MiB corpus.
//!   Measures cold-cache read throughput under contention (MUST show no
//!   regression — reads are untouched).

use bench_scale_tool::Harness;
use bytes::Bytes;
use shamir_storage::types::{RecordKey, Repo, Store};
use std::sync::Arc;

/// Value size per record. 2 KiB × 128k records ≈ 256 MiB total corpus.
const RECORD_SIZE: usize = 2 * 1024;

/// Records seeded before measuring. 128k × 2 KiB ≈ 256 MiB ≈ 8× the 32 MiB
/// default fjall block cache, forcing cold-cache reads.
const SEED_COUNT: usize = 128 * 1024;

/// Concurrent writers per timed write iteration.
const WRITE_FANOUT: usize = 32;

/// Concurrent readers per timed read iteration.
const READ_FANOUT: usize = 32;

fn make_value(i: usize) -> Bytes {
    let mut v = vec![0u8; RECORD_SIZE];
    v[..8].copy_from_slice(&(i as u64).to_be_bytes());
    for (j, b) in v[8..].iter_mut().enumerate() {
        *b = ((i as u64).wrapping_add(j as u64)) as u8;
    }
    Bytes::from(v)
}

/// Build a fresh tempdir-backed `FjallStore`. The tempdir is leaked so it
/// outlives the registered closures — the OS reclaims it on process exit.
fn make_fjall_store() -> Arc<dyn Store> {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = shamir_storage::storage_fjall::FjallRepo::new(dir.path()).unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = rt.block_on(repo.store_get("wworker")).unwrap();
    std::mem::forget(dir);
    store
}

/// Seed `n` records under fixed 16-byte big-endian keys `0..n`, then flush so
/// the corpus is on disk (out of the write buffer). Returns the keys.
async fn seed_large(store: &Arc<dyn Store>, n: usize) -> Vec<RecordKey> {
    let mut keys = Vec::with_capacity(n);
    for i in 0..n {
        // 16-byte key: 8 bytes zero prefix + 8 bytes big-endian index, so keys
        // are the same width as real `RecordId`s and sort in index order.
        let mut kb = [0u8; 16];
        kb[8..].copy_from_slice(&(i as u64).to_be_bytes());
        let key = RecordKey::from_slice(&kb);
        store.set(key.clone(), make_value(i)).await.unwrap();
        keys.push(key);
    }
    // Force segments to disk so subsequent reads are genuine cache misses.
    store.flush().await.unwrap();
    keys
}

/// A cheap xorshift so the read workload picks scattered keys without pulling
/// in an RNG crate.
fn next_rand(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// `read_cold_scattered` — each iteration issues `READ_FANOUT` concurrent
/// point-reads at scattered keys across the full 256 MiB corpus and awaits
/// them all. Reads are on the untouched `spawn_blocking` path; this workload
/// exists to prove no regression there.
fn bench_read_cold(h: &mut Harness, store: Arc<dyn Store>, keys: Arc<Vec<RecordKey>>) {
    let mut rng: u64 = 0x9E37_79B9_7F4A_7C15;
    h.bench_async("read_cold_scattered", move || {
        let s = Arc::clone(&store);
        let ks = Arc::clone(&keys);
        // Pick a fresh scattered base offset each iteration.
        let base = next_rand(&mut rng) as usize;
        async move {
            let mut handles = Vec::with_capacity(READ_FANOUT);
            for j in 0..READ_FANOUT {
                let s = Arc::clone(&s);
                let ks = Arc::clone(&ks);
                // Spread reads across the corpus with a large stride so
                // consecutive reads touch different segments/blocks.
                let idx = base.wrapping_add(j.wrapping_mul(4099)) % ks.len();
                let key = ks[idx].clone();
                handles.push(tokio::spawn(async move { s.get(key).await.unwrap() }));
            }
            for hnd in handles {
                std::hint::black_box(hnd.await.unwrap());
            }
        }
    });
}

/// `write_concurrent_set` — each iteration fans out `WRITE_FANOUT` concurrent
/// `set` calls (all overwriting existing keys, so each pays the `contains_key`
/// probe + insert) and awaits them all. This is the contention path the
/// write-worker targets.
fn bench_write_set(h: &mut Harness, store: Arc<dyn Store>, keys: Arc<Vec<RecordKey>>) {
    let mut rng: u64 = 0x2545_F491_4F6C_DD1D;
    h.bench_async("write_concurrent_set", move || {
        let s = Arc::clone(&store);
        let ks = Arc::clone(&keys);
        let base = next_rand(&mut rng) as usize;
        async move {
            let mut handles = Vec::with_capacity(WRITE_FANOUT);
            for j in 0..WRITE_FANOUT {
                let s = Arc::clone(&s);
                let ks = Arc::clone(&ks);
                let idx = base.wrapping_add(j.wrapping_mul(4099)) % ks.len();
                let key = ks[idx].clone();
                let val = make_value(idx ^ j);
                handles.push(tokio::spawn(async move { s.set(key, val).await.unwrap() }));
            }
            for hnd in handles {
                std::hint::black_box(hnd.await.unwrap());
            }
        }
    });
}

/// `write_concurrent_insert` — each iteration fans out `WRITE_FANOUT`
/// concurrent fresh-key `insert` calls and awaits them all. No `contains_key`
/// probe (fresh id), so this isolates pure write dispatch + journal contention.
fn bench_write_insert(h: &mut Harness, store: Arc<dyn Store>) {
    h.bench_async("write_concurrent_insert", move || {
        let s = Arc::clone(&store);
        async move {
            let mut handles = Vec::with_capacity(WRITE_FANOUT);
            for _ in 0..WRITE_FANOUT {
                let s = Arc::clone(&s);
                let val = make_value(0);
                handles.push(tokio::spawn(async move { s.insert(val).await.unwrap() }));
            }
            for hnd in handles {
                std::hint::black_box(hnd.await.unwrap());
            }
        }
    });
}

fn main() {
    let mut h = Harness::new("storage_fjall_write_worker", env!("CARGO_MANIFEST_DIR"));

    // One big shared corpus for reads + set-contention; a separate store for
    // insert so its fresh-key growth doesn't perturb the read corpus.
    let read_store = make_fjall_store();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let keys = Arc::new(rt.block_on(seed_large(&read_store, SEED_COUNT)));

    bench_read_cold(&mut h, Arc::clone(&read_store), Arc::clone(&keys));
    bench_write_set(&mut h, Arc::clone(&read_store), Arc::clone(&keys));
    bench_write_insert(&mut h, make_fjall_store());

    h.run();
}
