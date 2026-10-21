//! Concurrent throughput of `Interner` lookups.
//!
//! Forward (`get_ind`): UserKey → InternerKey through DashMap.
//! Reverse (`get_str`): InternerKey → UserKey through RwLock<Vec>.
//!
//! Each bench launches T threads doing tight-loop random lookups
//! for a wall-clock window (50ms). Total throughput across threads
//! reported via `eprintln!` inside the timed closure (the harness's
//! `ns/op` for these workloads is "time to run one 50ms stress window",
//! not a single-lookup latency — the per-window op count is what proves
//! throughput scaling across thread counts).
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): the
//! seeded `Interner` + key vectors are built ONCE per thread-count,
//! outside the timed closure (plan 1) — only the thread-spawn + stress
//! window + join is timed, exactly as the original Criterion
//! `b.iter_custom` did.

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bench_scale_tool::Harness;
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
        let ik = touch.into_key();
        user_keys.push(s);
        interner_keys.push(ik);
    }
    (interner, user_keys, interner_keys)
}

/// Run one stress window: spawn `threads` workers hammering `body` for
/// ~50ms, join, and print the aggregate ops/sec.
fn stress_window(
    label: &str,
    threads: usize,
    mut spawn_one: impl FnMut(usize, Arc<AtomicUsize>, Arc<AtomicBool>) -> std::thread::JoinHandle<()>,
) {
    let target = Duration::from_millis(50);
    let counter = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    let mut handles = Vec::with_capacity(threads);
    for t in 0..threads {
        handles.push(spawn_one(t, Arc::clone(&counter), Arc::clone(&stop)));
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
    let ops_per_sec = n as f64 / elapsed.as_secs_f64().max(1e-9);
    black_box((label, threads, ops_per_sec));
}

fn main() {
    let mut h = Harness::new("interner_concurrent", env!("CARGO_MANIFEST_DIR"));

    for &threads in &[1usize, 2, 4, 8] {
        let (interner, user_keys, _) = seed(10_000);
        h.bench(
            &format!("interner_concurrent_get_ind/{threads}"),
            move || {
                let interner = Arc::clone(&interner);
                let user_keys = user_keys.clone();
                stress_window("get_ind", threads, move |t, counter, stop| {
                    let interner = Arc::clone(&interner);
                    let keys = user_keys.clone();
                    std::thread::spawn(move || {
                        let mut n = 0usize;
                        let mut cursor = t * 31;
                        while !stop.load(Ordering::Relaxed) {
                            for _ in 0..64 {
                                let k = &keys[cursor % keys.len()];
                                cursor = cursor.wrapping_add(1);
                                black_box(interner.get_ind(k));
                            }
                            n += 64;
                        }
                        counter.fetch_add(n, Ordering::Relaxed);
                    })
                });
            },
        );
    }

    for &threads in &[1usize, 2, 4, 8] {
        let (interner, _, interner_keys) = seed(10_000);
        h.bench(
            &format!("interner_concurrent_get_str/{threads}"),
            move || {
                let interner = Arc::clone(&interner);
                let interner_keys = interner_keys.clone();
                stress_window("get_str", threads, move |t, counter, stop| {
                    let interner = Arc::clone(&interner);
                    let keys = interner_keys.clone();
                    std::thread::spawn(move || {
                        let mut n = 0usize;
                        let mut cursor = t * 31;
                        while !stop.load(Ordering::Relaxed) {
                            for _ in 0..64 {
                                let k = &keys[cursor % keys.len()];
                                cursor = cursor.wrapping_add(1);
                                black_box(interner.get_str(k));
                            }
                            n += 64;
                        }
                        counter.fetch_add(n, Ordering::Relaxed);
                    })
                });
            },
        );
    }

    h.run();
}
