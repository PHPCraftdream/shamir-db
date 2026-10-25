//! Regression tests for realloc accounting and opt-in allocator wiring.

use std::alloc::{GlobalAlloc, Layout};
use std::thread;

use crate::peak_mem::{current_allocated, current_peak, reset, PeakTracker};

/// Realloc must account `new - old` atomically through the REAL allocator
/// entry point: the peak never reaches the `old + new` spike of an
/// allocate-copy-deallocate realloc, a shrink never underflows, and
/// concurrent alloc/realloc/dealloc loops leave `CURRENT` exactly where it
/// started (a load-then-store realloc loses concurrent updates and the
/// final `fetch_sub`s then underflow the counter).
#[test]
fn rv_bench_utils_e6177532_realloc_accounting_is_atomic_and_has_no_spike() {
    let tracker = PeakTracker::new();
    let small = Layout::from_size_align(100, 8).unwrap();

    // Single thread: grow 100 -> 300 must peak at base + 300, not base + 400.
    let base = current_allocated();
    reset();
    unsafe {
        let p = tracker.alloc(small);
        assert!(!p.is_null());
        let p = tracker.realloc(p, small, 300);
        assert!(!p.is_null());
        assert_eq!(current_allocated(), base + 300);
        assert_eq!(current_peak(), base + 300, "no old+new spike");
        let big = Layout::from_size_align(300, 8).unwrap();
        let p = tracker.realloc(p, big, 100);
        assert_eq!(current_allocated(), base + 100, "shrink accounts new-old");
        tracker.dealloc(p, small);
    }
    assert_eq!(current_allocated(), base);

    // Concurrent: balanced loops must return CURRENT to base exactly.
    let handles: Vec<_> = (0..8)
        .map(|_| {
            thread::spawn(|| {
                let tracker = PeakTracker::new();
                let small = Layout::from_size_align(64, 8).unwrap();
                for _ in 0..2_000 {
                    unsafe {
                        let p = tracker.alloc(small);
                        let p = tracker.realloc(p, small, 256);
                        let grown = Layout::from_size_align(256, 8).unwrap();
                        let p = tracker.realloc(p, grown, 32);
                        tracker.dealloc(p, Layout::from_size_align(32, 8).unwrap());
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(
        current_allocated(),
        base,
        "balanced concurrent ops must leave the live counter untouched"
    );
}

/// The dev-dependencies must link only the fixture generators; the tracking
/// allocator is installed exclusively by an explicit `install_peak_alloc!()`
/// in a measuring binary. (Fails on the unfixed code, where both manifests
/// contain `features = ["peak_mem"]`.)
#[test]
fn rv_bench_utils_e1f3d036_no_dev_dependency_enables_peak_mem() {
    for crate_name in ["shamir-engine", "shamir-index"] {
        let path = format!("{}/../{crate_name}/Cargo.toml", env!("CARGO_MANIFEST_DIR"));
        let manifest =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
        assert!(
            !manifest.contains("\"peak_mem"),
            "{crate_name} Cargo.toml must not enable the peak_mem feature"
        );
        assert!(
            !manifest.contains("peak_mem\"]"),
            "{crate_name} Cargo.toml must not enable the peak_mem feature"
        );
    }
}
