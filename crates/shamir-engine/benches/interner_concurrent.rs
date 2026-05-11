//! Concurrent throughput of `Interner` lookups.
//!
//! Forward (`get_ind`): UserKey → InternerKey through DashMap.
//! Reverse (`get_str`): InternerKey → UserKey through RwLock<Vec>.
//!
//! Each bench launches T threads doing tight-loop random lookups
//! for a wall-clock window. Total throughput across threads
//! reported.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput,
};

use shamir_types::core::interner::{Interner, InternerKey};

/// Seed N pre-interned strings, returning (interner, user_keys,
/// interner_keys).
fn seed(n: usize) -> (Arc<Interner>, Vec<String>, Vec<InternerKey>) {
    let interner = Arc::new(Interner::new());
    let mut user_keys = Vec::with_capacity(n);
    let mut interner_keys = Vec::with_capacity(n);
    for i in 0..n {
        let s = format!("user_key_{i}");
        let touch = interner.touch_ind(&s).expect("touch");
        let ik = touch.key().clone();
        user_keys.push(s);
        interner_keys.push(ik);
    }
    (interner, user_keys, interner_keys)
}

fn bench_concurrent_get_ind(c: &mut Criterion) {
    let (interner, user_keys, _) = seed(10_000);
    let mut group = c.benchmark_group("interner_concurrent_get_ind");

    for &threads in &[1usize, 2, 4, 8] {
        group.throughput(Throughput::Elements(threads as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(threads),
            &threads,
            |b, &threads| {
                let interner = Arc::clone(&interner);
                let user_keys = user_keys.clone();
                b.iter_custom(|iters| {
                    let target = Duration::from_millis(50);
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let counter = Arc::new(AtomicUsize::new(0));
                        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
                        let mut handles = Vec::with_capacity(threads);
                        let start = Instant::now();
                        for t in 0..threads {
                            let interner = Arc::clone(&interner);
                            let keys = user_keys.clone();
                            let c = Arc::clone(&counter);
                            let s = Arc::clone(&stop);
                            handles.push(std::thread::spawn(move || {
                                let mut n = 0usize;
                                let mut cursor = t * 31;
                                while !s.load(Ordering::Relaxed) {
                                    for _ in 0..64 {
                                        let k = &keys[cursor % keys.len()];
                                        cursor = cursor.wrapping_add(1);
                                        black_box(interner.get_ind(k));
                                    }
                                    n += 64;
                                }
                                c.fetch_add(n, Ordering::Relaxed);
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
                            // Time per "iter" = elapsed; throughput is
                            // total ops / elapsed. Report time / (n /
                            // threads) — criterion divides by threads.
                            total += elapsed / (n as u32 / threads as u32).max(1);
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

fn bench_concurrent_get_str(c: &mut Criterion) {
    let (interner, _, interner_keys) = seed(10_000);
    let mut group = c.benchmark_group("interner_concurrent_get_str");

    for &threads in &[1usize, 2, 4, 8] {
        group.throughput(Throughput::Elements(threads as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(threads),
            &threads,
            |b, &threads| {
                let interner = Arc::clone(&interner);
                let interner_keys = interner_keys.clone();
                b.iter_custom(|iters| {
                    let target = Duration::from_millis(50);
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let counter = Arc::new(AtomicUsize::new(0));
                        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
                        let mut handles = Vec::with_capacity(threads);
                        let start = Instant::now();
                        for t in 0..threads {
                            let interner = Arc::clone(&interner);
                            let keys = interner_keys.clone();
                            let c = Arc::clone(&counter);
                            let s = Arc::clone(&stop);
                            handles.push(std::thread::spawn(move || {
                                let mut n = 0usize;
                                let mut cursor = t * 31;
                                while !s.load(Ordering::Relaxed) {
                                    for _ in 0..64 {
                                        let k = &keys[cursor % keys.len()];
                                        cursor = cursor.wrapping_add(1);
                                        black_box(interner.get_str(k));
                                    }
                                    n += 64;
                                }
                                c.fetch_add(n, Ordering::Relaxed);
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
                            total += elapsed / (n as u32 / threads as u32).max(1);
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

criterion_group!(benches, bench_concurrent_get_ind, bench_concurrent_get_str);
criterion_main!(benches);
