//! Stage 0 bench for the hidden-O(N) sweep campaign.
//!
//! Measures the cost of `VersionedOverlay::gc_upto(threshold)` as a
//! function of overlay depth. Today (HEAD) `gc_upto` walks the full
//! `(key, version)`-ordered B+ tree filtering by version — that is
//! O(total entries), not O(removed). The drainer calls it on EVERY
//! pass (drainer.rs: `per_table_mvcc.scan(|_, mvcc| mvcc.gc_overlay_to(durable))`).
//!
//! If the bench shows flat per-depth time, the O(N) cliff is theoretical
//! only (Op #2 keeps the overlay window tight); if it scales with depth,
//! Stage 1 (version-major index) is justified.
//!
//! Each cell:
//!  1. Build a fresh overlay (sync, no async, no I/O).
//!  2. Pre-fill N=DEPTH entries via plain `insert`, distinct keys, monotone versions.
//!  3. Time ONE `gc_upto(durable=DEPTH, floor=u64::MAX)` over the full depth.
//!  4. Drop and rebuild between iterations (single-shot timing, not iterated).
//!
//! Quick mode (default): 10 samples, 1s measurement, 1s warm-up, 60s wall-clock cap.

use bytes::Bytes;
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use shamir_bench_utils::tune_tiered;
use shamir_tx::VersionedOverlay;

const DEPTHS: &[usize] = &[1_000, 5_000, 20_000];

fn build_overlay(depth: usize) -> VersionedOverlay {
    let ov = VersionedOverlay::new();
    for i in 0..depth {
        let key = Bytes::copy_from_slice(&(i as u64).to_be_bytes());
        let value = Bytes::from_static(b"v");
        // One distinct version per entry — gc_upto(depth) must scan ALL.
        ov.insert(key, (i + 1) as u64, value);
    }
    ov
}

fn bench_overlay_gc_full_purge(c: &mut Criterion) {
    let mut group = c.benchmark_group("overlay_gc_full_purge");
    // Each "iter" measures one GC over a freshly-built overlay; the build
    // is done in iter_batched setup so it does NOT count toward the timing.
    tune_tiered(&mut group, 20, 5, 3, 600);

    for &depth in DEPTHS {
        group.throughput(Throughput::Elements(depth as u64));
        group.bench_function(format!("depth_{depth}"), |b| {
            b.iter_batched(
                || build_overlay(depth),
                |ov| {
                    // Drop EVERY entry — threshold = depth covers all versions.
                    ov.gc_upto(depth as u64, u64::MAX);
                    ov
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_overlay_gc_small_slice(c: &mut Criterion) {
    let mut group = c.benchmark_group("overlay_gc_small_slice");
    tune_tiered(&mut group, 20, 5, 3, 600);

    // Same depths, but only the lowest 100 versions removed per GC. The
    // adversarial case for the current full-iter implementation: we pay
    // for the whole tree to remove a tiny slice. Stage 1's version-major
    // index makes this O(removed + log N) instead.
    const SLICE: u64 = 100;

    for &depth in DEPTHS {
        group.throughput(Throughput::Elements(SLICE));
        group.bench_function(format!("depth_{depth}_slice_{SLICE}"), |b| {
            b.iter_batched(
                || build_overlay(depth),
                |ov| {
                    ov.gc_upto(SLICE, u64::MAX);
                    ov
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_overlay_gc_full_purge,
    bench_overlay_gc_small_slice
);
criterion_main!(benches);
