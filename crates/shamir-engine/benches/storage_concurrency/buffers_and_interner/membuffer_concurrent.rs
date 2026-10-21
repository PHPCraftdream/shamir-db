//! Concurrent reader+writer bench for `MemBufferStore`.
//!
//! Setup: warm cache holding 1000 entries. N reader-tasks loop
//! `store.get(random_key)`; 1 writer-task loops `store.set(...)`.
//! Bench measures total ops/sec for varying reader counts (1, 2,
//! 4, 8) over a fixed 50ms stress window. If `cache.lock()` is
//! contended, writer latency or reader throughput will plateau as
//! readers scale.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): the
//! warm `MemBufferStore` + seeded keys are built ONCE per reader-count,
//! outside the timed closure (plan 1) — only the thread-spawn + stress
//! window + join is timed, exactly as the original Criterion
//! `b.iter_custom` did. Each reader/writer thread owns its own
//! single-thread tokio runtime (mirrors the original — the store's async
//! API is driven from plain OS threads, not the harness's shared runtime).

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bench_scale_tool::Harness;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;

fn build_warm_store() -> (Arc<dyn Store>, Vec<RecordKey>) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let cfg = MemBufferConfig {
            max_bytes: 64 * 1024 * 1024,
            max_entries: 100_000,
            ttl_ms: None,
            flush_interval_ms: 60_000,
            flush_batch_size: 256,
        };
        let s: Arc<dyn Store> = Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
        let mut ks = Vec::with_capacity(1000);
        for _ in 0..1000 {
            let id = RecordId::new();
            let k = RecordKey::from_slice(id.as_bytes());
            let v = Bytes::copy_from_slice(b"value-100-bytes-...-padding-padding-padding-padding-padding-padding-padding-padding-padding-pad");
            s.set(k.clone(), v).await.unwrap();
            ks.push(k);
        }
        (s, ks)
    })
}

fn main() {
    let mut h = Harness::new("membuffer_concurrent", env!("CARGO_MANIFEST_DIR"));

    for &readers in &[1usize, 2, 4, 8] {
        let (store, keys) = build_warm_store();
        h.bench(&format!("membuffer_concurrent_rw/{readers}"), move || {
            let target = Duration::from_millis(50);
            let counter = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let mut handles = Vec::with_capacity(readers + 1);
            let start = Instant::now();

            // Reader tasks.
            for t in 0..readers {
                let s = Arc::clone(&store);
                let ks = keys.clone();
                let c = Arc::clone(&counter);
                let stop_flag = Arc::clone(&stop);
                handles.push(std::thread::spawn(move || {
                    let local_rt = tokio::runtime::Runtime::new().unwrap();
                    local_rt.block_on(async move {
                        let mut n = 0usize;
                        let mut cursor = t * 31usize;
                        while !stop_flag.load(Ordering::Relaxed) {
                            for _ in 0..16 {
                                let key = ks[cursor % ks.len()].clone();
                                cursor = cursor.wrapping_add(1);
                                black_box(s.get(key).await.ok());
                            }
                            n += 16;
                        }
                        c.fetch_add(n, Ordering::Relaxed);
                    });
                }));
            }

            // One writer task.
            {
                let s = Arc::clone(&store);
                let ks = keys.clone();
                let c = Arc::clone(&counter);
                let stop_flag = Arc::clone(&stop);
                handles.push(std::thread::spawn(move || {
                    let local_rt = tokio::runtime::Runtime::new().unwrap();
                    local_rt.block_on(async move {
                        let v = Bytes::copy_from_slice(b"new-value-padding-padding-padding-padding-padding-padding-padding-padding-padding-padding-padding");
                        let mut n = 0usize;
                        let mut cursor = 7usize;
                        while !stop_flag.load(Ordering::Relaxed) {
                            for _ in 0..16 {
                                let key = ks[cursor % ks.len()].clone();
                                cursor = cursor.wrapping_add(17);
                                black_box(s.set(key, v.clone()).await.ok());
                            }
                            n += 16;
                        }
                        c.fetch_add(n, Ordering::Relaxed);
                    });
                }));
            }

            while start.elapsed() < target {
                std::thread::yield_now();
            }
            stop.store(true, Ordering::Relaxed);
            for handle in handles {
                handle.join().unwrap();
            }
            let elapsed = start.elapsed();
            let n = counter.load(Ordering::Relaxed);
            let ops_per_sec = n as f64 / elapsed.as_secs_f64().max(1e-9);
            black_box(ops_per_sec);
        });
    }

    h.run();
}
