//! `MemBufferStore` scan-under-writes pump — audit finding 2.3 (task #530).
//!
//! Run: `cargo bench -p shamir-storage --bench membuffer_scan_under_writes`
//!
//! The existing `membuffer_pump` / `store_raw` benches only measure
//! insert/get. NONE covers a SCAN running against a MemBuffer that still has a
//! dirty write buffer — which is exactly the path the audit flags: before the
//! fix, EVERY `iter_stream` / `scan_prefix_stream` / `iter_range_stream(_reverse)`
//! looped `drain_once` until the dirty buffer was fully flushed to `inner`
//! BEFORE streaming — read-triggered write amplification that also defeats the
//! 500ms fsync-batching interval on every scan.
//!
//! After the fix the scan snapshots the (small) dirty overlay and MERGES it on
//! top of the sorted `inner` stream — no flush required to read.
//!
//! # Workload
//!
//! Inner store = a real tempdir-backed fjall instance, so the drain-to-disk
//! cost the old path paid is REAL (not an in-RAM no-op). Each timed iteration:
//!   1. write a batch of `DIRTY_BATCH` fresh records into the buffer (they
//!      land in the dirty overlay, not yet flushed), then
//!   2. run a full `iter_stream` scan and drain it.
//!
//! On the OLD (drain-before-scan) code, step 2 force-flushes step 1's writes to
//! fjall on every iteration (write amplification). On the NEW (merge-overlay)
//! code, step 2 merges the overlay in RAM and leaves it dirty for the periodic
//! flusher to batch — the fsync amortisation the buffer exists to provide.
//!
//! `scan_only` is a control: a scan over a buffer whose dirty overlay is empty
//! (already flushed) — both code paths behave identically here, so it pins the
//! baseline scan cost and isolates the amplification delta in `scan_with_dirty`.

use bench_scale_tool::Harness;
use bytes::Bytes;
use shamir_storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
use shamir_storage::types::{Repo, Store};
use std::sync::Arc;

/// Record payload size — modest so the tempdir stays in page cache (we are
/// measuring the drain/flush + merge CPU cost, not raw disk bandwidth).
const RECORD_SIZE: usize = 256;
/// How many durable records the inner store is seeded with (the LARGE side of
/// the merge — the sorted `inner` stream the scan walks every time).
const SEED_COUNT: usize = 2_000;
/// How many fresh dirty records each iteration writes before scanning (the
/// SMALL overlay side). This is the write batch the old code would flush on
/// every scan.
const DIRTY_BATCH: usize = 64;

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

/// Build a fresh tempdir-backed fjall inner store wrapped in a
/// `MemBufferStore`. The tempdir is leaked so it outlives the closures.
///
/// `MemBufferStore::new` spawns a background flusher via `tokio::spawn`, so it
/// MUST be constructed inside a runtime context — `rt` is the runtime the
/// caller will also use for the async workload.
fn make_buffered_store(rt: &tokio::runtime::Runtime) -> Arc<MemBufferStore> {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = shamir_storage::storage_fjall::FjallRepo::new(dir.path()).unwrap();
    let inner = rt.block_on(repo.store_get("scan")).unwrap();
    std::mem::forget(dir);
    // Long flush interval so the background flusher does NOT drain during the
    // measured window — the scan itself is the only thing that could flush,
    // which is exactly the behaviour under test.
    let cfg = MemBufferConfig {
        max_bytes: 256 * 1024 * 1024,
        max_entries: 5_000_000,
        ttl_ms: None,
        flush_interval_ms: 600_000,
        flush_batch_size: 256,
    };
    rt.block_on(async { Arc::new(MemBufferStore::new(inner, cfg)) })
}

async fn seed(store: &Arc<MemBufferStore>, n: usize) {
    for i in 0..n {
        store.insert(make_value(i)).await.unwrap();
    }
    // Flush so the seed corpus is durable in `inner` (the large merge side)
    // and the dirty overlay starts empty.
    store.flush().await.unwrap();
}

async fn full_scan(store: &Arc<MemBufferStore>) -> u64 {
    use futures::StreamExt;
    let mut stream = store.iter_stream(256);
    let mut count = 0u64;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len() as u64;
    }
    count
}

/// `scan_with_dirty` — the finding under test: write a batch of dirty records,
/// then scan. Old code flushes the batch on every scan; new code merges it.
fn bench_scan_with_dirty(
    h: &mut Harness,
    rt: &tokio::runtime::Runtime,
    store: Arc<MemBufferStore>,
) {
    rt.block_on(seed(&store, SEED_COUNT));
    let counter = std::sync::atomic::AtomicUsize::new(0);
    h.bench_async("scan_with_dirty", move || {
        let s = Arc::clone(&store);
        // Distinct key base per iteration so each scan sees a fresh dirty set.
        let base = counter.fetch_add(DIRTY_BATCH, std::sync::atomic::Ordering::Relaxed);
        async move {
            for j in 0..DIRTY_BATCH {
                s.set(
                    shamir_storage::types::RecordKey::from_slice(
                        &((base + j) as u64).to_be_bytes(),
                    ),
                    make_value(base + j),
                )
                .await
                .unwrap();
            }
            let n = full_scan(&s).await;
            std::hint::black_box(n);
        }
    });
}

/// `scan_only` — control: scan over an already-flushed (empty overlay) buffer.
/// Both old and new code behave identically here; pins baseline scan cost.
fn bench_scan_only(h: &mut Harness, rt: &tokio::runtime::Runtime, store: Arc<MemBufferStore>) {
    rt.block_on(seed(&store, SEED_COUNT));
    h.bench_async("scan_only", move || {
        let s = Arc::clone(&store);
        async move {
            let n = full_scan(&s).await;
            std::hint::black_box(n);
        }
    });
}

fn main() {
    let mut h = Harness::new("membuffer_scan_under_writes", env!("CARGO_MANIFEST_DIR"));
    // Single shared runtime: constructs the stores (spawning their background
    // flushers) AND — via `Harness::bench_async` — drives the measured async
    // workloads, so the fjall `spawn_blocking` calls always see a live reactor.
    let rt = rt();
    let s_only = make_buffered_store(&rt);
    let s_dirty = make_buffered_store(&rt);
    bench_scan_only(&mut h, &rt, s_only);
    bench_scan_with_dirty(&mut h, &rt, s_dirty);
    h.run();
}
