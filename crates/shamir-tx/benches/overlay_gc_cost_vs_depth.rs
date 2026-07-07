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
//!  1. Build a fresh overlay (sync, no async, no I/O) — untimed setup.
//!  2. Pre-fill N=DEPTH entries via plain `insert`, distinct keys, monotone versions.
//!  3. Time ONE `gc_upto(durable=DEPTH, floor=u64::MAX)` over the full depth.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): the overlay
//! MUST be rebuilt fresh every iteration (`gc_upto` drains/mutates it, so a
//! shared overlay would only be gc'able once), so every variant uses
//! `bench_batched` — the fresh build is the untimed `setup`, only `gc_upto`
//! is timed.

use std::hint::black_box;

use bench_scale_tool::Harness;
use bytes::Bytes;
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

fn main() {
    let mut h = Harness::new("overlay_gc_cost_vs_depth", env!("CARGO_MANIFEST_DIR"));

    // --- overlay_gc_full_purge/depth_<depth> --------------------------------
    for &depth in DEPTHS {
        let id = format!("overlay_gc_full_purge/depth_{depth}");
        h.bench_batched(
            &id,
            move || build_overlay(depth),
            move |ov| {
                // Drop EVERY entry — threshold = depth covers all versions.
                ov.gc_upto(depth as u64, u64::MAX);
                black_box(&ov);
            },
        );
    }

    // --- overlay_gc_small_slice/depth_<depth>_slice_<SLICE> -----------------
    // Same depths, but only the lowest 100 versions removed per GC. The
    // adversarial case for the current full-iter implementation: we pay for
    // the whole tree to remove a tiny slice. Stage 1's version-major index
    // makes this O(removed + log N) instead.
    const SLICE: u64 = 100;
    for &depth in DEPTHS {
        let id = format!("overlay_gc_small_slice/depth_{depth}_slice_{SLICE}");
        h.bench_batched(
            &id,
            move || build_overlay(depth),
            move |ov| {
                ov.gc_upto(SLICE, u64::MAX);
                black_box(&ov);
            },
        );
    }

    h.run();
}
