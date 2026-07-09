//! Dedicated `FjallStore` pump — point `get`/`insert`/`set`/`scan_prefix`
//! against a real tempdir-backed fjall instance.
//!
//! Run: `cargo bench -p shamir-storage --bench storage_fjall_pump`
//!
//! `store_raw::bench_fjall` already exercises fjall end-to-end, but it (a)
//! mixes fjall with the in-memory backends in the same binary, and (b) has
//! no single-key `set` (existing-key) variant — which is exactly the path
//! the audit's finding 1.2 flags as paying a redundant `contains_key` LSM
//! point-lookup before every mutation. This bench isolates that path so the
//! before/after signal for the zero-copy read + double-lookup-removal /opti
//! (audit `2026-07-06-perf-radical-o-notation` §1.1/1.2) is clean.
//!
//! # Workloads
//!
//! - `get` — repeated point-read of an existing key (exercises the read
//!   path; finding 1.1's `Bytes::copy_from_slice` cost).
//! - `insert` — fresh random-key insert (finding 1.2: `insert`'s
//!   `contains_key` check is provably pointless — fresh `RecordId`).
//! - `set_existing` — `set` on a key that already exists (finding 1.2:
//!   the `existed`-flag `contains_key` check).
//! - `scan_prefix` — small prefix scan (exercises both key + value
//!   zero-copy conversions in the stream path).

use bench_scale_tool::Harness;
use bytes::Bytes;
use shamir_storage::types::{RecordKey, Repo, Store};
use std::sync::Arc;

/// Record size — large enough that the memcpy in `copy_from_slice` is a
/// measurable fraction of the per-op cost (so the zero-copy win is
/// visible), small enough that the tempdir stays in the OS page cache
/// (we are measuring CPU/alloc cost, not disk I/O).
const RECORD_SIZE: usize = 512;

/// How many records the read/scan workloads seed before measuring.
/// Kept modest so the bench completes in seconds in QUICK mode.
const SEED_COUNT: usize = 2_000;

fn make_value(i: usize) -> Bytes {
    let mut v = vec![0u8; RECORD_SIZE];
    v[..8].copy_from_slice(&(i as u64).to_be_bytes());
    // Fill the rest with a cheap pattern so the buffer isn't trivially
    // compressible / dedupable by the allocator.
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

/// Build a fresh tempdir-backed `FjallStore`. The tempdir is deliberately
/// leaked (`std::mem::forget`) so it outlives the registered closures —
/// the OS reclaims it on bench exit.
fn make_fjall_store() -> Arc<dyn Store> {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = shamir_storage::storage_fjall::FjallRepo::new(dir.path()).unwrap();
    let rt = rt();
    let store = rt.block_on(repo.store_get("pump")).unwrap();
    std::mem::forget(dir);
    store
}

/// Seed `n` fresh random-key records into `store`, returning the generated
/// keys (in insertion order).
async fn seed(store: &Arc<dyn Store>, n: usize) -> Vec<RecordKey> {
    let mut keys = Vec::with_capacity(n);
    for i in 0..n {
        keys.push(store.insert(make_value(i)).await.unwrap());
    }
    keys
}

/// `get` — repeated point-read of ONE existing key. The seed runs once
/// (outside the timed closure); the closure reads the same key every
/// iteration so we measure pure read-path cost (the audit's finding 1.1
/// memcpy + alloc).
fn bench_get(h: &mut Harness, store: Arc<dyn Store>) {
    let rt = rt();
    let keys = rt.block_on(seed(&store, SEED_COUNT));
    let key = keys[SEED_COUNT / 2].clone();
    h.bench_async("get", move || {
        let s = Arc::clone(&store);
        let k = key.clone();
        async move {
            let b = s.get(k).await.unwrap();
            std::hint::black_box(b);
        }
    });
}

/// `insert` — fresh random-key insert per iteration. Exercises finding 1.2
/// for the `insert` path (the `contains_key` check that is provably
/// pointless because `RecordId::new()` is a fresh 128-bit random).
fn bench_insert(h: &mut Harness, store: Arc<dyn Store>) {
    h.bench_async("insert", move || {
        let s = Arc::clone(&store);
        async move {
            let k = s.insert(make_value(0)).await.unwrap();
            std::hint::black_box(k);
        }
    });
}

/// `set_existing` — `set` on a key that already exists, every iteration.
/// This is the path that pays the redundant `contains_key` LSM point-lookup
/// to report the `existed` flag (finding 1.2 for `set`). We re-`set` the
/// SAME key every iteration so the key always exists — the worst case for
/// the double-lookup cost.
fn bench_set_existing(h: &mut Harness, store: Arc<dyn Store>) {
    let rt = rt();
    // Seed one key; every iteration overwrites it.
    let key = rt.block_on(seed(&store, 1)).pop().unwrap();
    let val = make_value(42);
    h.bench_async("set_existing", move || {
        let s = Arc::clone(&store);
        let k = key.clone();
        let v = val.clone();
        async move {
            let created = s.set(k, v).await.unwrap();
            // `created` should be false on every iteration after the first
            // (the key already exists) — black_box it so the compiler
            // can't elide the call.
            std::hint::black_box(created);
        }
    });
}

/// `scan_prefix` — small prefix scan. Each iteration opens a fresh stream
/// over a shared-prefix corpus and drains it. Exercises the zero-copy key
/// AND value conversions in the stream path (finding 1.1).
fn bench_scan_prefix(h: &mut Harness, store: Arc<dyn Store>) {
    const PREFIX_BYTES: &[u8] = b"pfxpump_";
    const SCAN_N: usize = 512;

    let rt = rt();
    rt.block_on(async {
        for i in 0..SCAN_N {
            let mut key = Vec::with_capacity(16);
            key.extend_from_slice(PREFIX_BYTES);
            key.extend_from_slice(&(i as u64).to_be_bytes());
            let rk = RecordKey::from(key);
            store.set(rk, make_value(i)).await.unwrap();
        }
    });

    let prefix = Bytes::from_static(PREFIX_BYTES);
    h.bench_async("scan_prefix", move || {
        let s = Arc::clone(&store);
        let pfx = prefix.clone();
        async move {
            use futures::StreamExt;
            let mut stream = s.scan_prefix_stream(pfx, 256);
            let mut count = 0u64;
            while let Some(batch) = stream.next().await {
                count += batch.unwrap().len() as u64;
            }
            std::hint::black_box(count);
        }
    });
}

fn main() {
    let mut h = Harness::new("storage_fjall_pump", env!("CARGO_MANIFEST_DIR"));

    // Each workload gets its OWN fresh store so seeds don't interfere and
    // the corpus shape matches what the workload expects (e.g. `get` wants
    // many keys; `set_existing` wants exactly one).
    bench_insert(&mut h, make_fjall_store());
    bench_get(&mut h, make_fjall_store());
    bench_set_existing(&mut h, make_fjall_store());
    bench_scan_prefix(&mut h, make_fjall_store());

    h.run();
}
