//! Micro-bench for `Histogram::observe` — design §5 acceptance: ≤ 50 ns/op
//! (one clock-free update; the bare `fetch_add` cell is the reference floor).

use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};

use bench_scale_tool::Harness;
use shamir_metrics::{Histogram, LATENCY_NS_BOUNDS};

fn main() {
    let mut h = Harness::new("histogram_observe", env!("CARGO_MANIFEST_DIR"));
    // The harness requires `'static` closures — leak the shared histogram
    // (one instance for the whole process, matching its engine placement).
    let hist: &'static Histogram = Box::leak(Box::new(Histogram::new(LATENCY_NS_BOUNDS)));
    // One histogram shared across iterations — steady-state hot path, no
    // per-iter construction noise. Linear-scan position differs per cell.
    for (name, v) in [
        ("low", 50_000u64),     // first bound (scan exits early)
        ("mid", 5_000_000),     // middle of the set
        ("overflow", u64::MAX), // full scan, overflow cell
    ] {
        let id = format!("histogram_observe/observe_{name}");
        h.bench(&id, move || {
            hist.observe(black_box(v));
        });
    }
    // Reference: the cheapest update the histogram can beat — one Relaxed
    // fetch_add, no bucket search, no snapshot counters.
    let bare: &'static AtomicU64 = Box::leak(Box::new(AtomicU64::new(0)));
    h.bench("histogram_observe/fetch_add_ref", move || {
        bare.fetch_add(1, Ordering::Relaxed);
    });
    h.run();
}
