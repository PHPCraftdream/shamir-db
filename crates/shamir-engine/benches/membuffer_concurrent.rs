//! Concurrent reader+writer bench for `MemBufferStore`.
//!
//! Setup: warm cache holding 1000 entries. N reader-tasks loop
//! `store.get(random_key)`; 1 writer-task loops `store.set(...)`.
//! Bench measures total ops/sec for varying reader counts (1, 2,
//! 4, 8). If `cache.lock()` is contended, writer latency or
//! reader throughput will plateau as readers scale.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput,
};
use tokio::runtime::Runtime;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;

fn bench_concurrent_rw(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("membuffer_concurrent_rw");
    group.sample_size(10);

    for &readers in &[1usize, 2, 4, 8] {
        group.throughput(Throughput::Elements((readers + 1) as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(readers),
            &readers,
            |b, &readers| {
                // Build warm cache once per bench point.
                let (store, keys): (Arc<dyn Store>, Vec<RecordKey>) =
                    rt.block_on(async {
                        let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                        let cfg = MemBufferConfig {
                            max_bytes: 64 * 1024 * 1024,
                            max_entries: 100_000,
                            ttl_ms: None,
                            flush_interval_ms: 60_000,
                            flush_batch_size: 256,
                        };
                        let s: Arc<dyn Store> =
                            Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                        let mut ks = Vec::with_capacity(1000);
                        for _ in 0..1000 {
                            let id = RecordId::new();
                            let k = RecordKey::copy_from_slice(id.as_bytes());
                            let v = RecordKey::copy_from_slice(b"value-100-bytes-...-padding-padding-padding-padding-padding-padding-padding-padding-padding-pad");
                            s.set(k.clone(), v).await.unwrap();
                            ks.push(k);
                        }
                        (s, ks)
                    });

                b.iter_custom(|iters| {
                    let target = Duration::from_millis(50);
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let counter = Arc::new(AtomicUsize::new(0));
                        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
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
                                    let v = RecordKey::copy_from_slice(b"new-value-padding-padding-padding-padding-padding-padding-padding-padding-padding-padding-padding");
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
                        for h in handles {
                            h.join().unwrap();
                        }
                        let elapsed = start.elapsed();
                        let n = counter.load(Ordering::Relaxed);
                        if n > 0 {
                            // Time per "iter" — criterion scales by
                            // throughput count (readers+1).
                            total += elapsed / (n as u32 / (readers + 1) as u32).max(1);
                        } else {
                            total += elapsed;
                        }
                    }
                    total
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_concurrent_rw);
criterion_main!(benches);
