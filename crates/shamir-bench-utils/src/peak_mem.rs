//! Peak memory allocation tracking for benchmarks.
//!
//! The tracking allocator is **NEVER installed by linking this module**.
//! A measuring binary opts in explicitly with [`install_peak_alloc!`] at
//! its crate root; every other binary that links this crate keeps its own
//! allocator untouched. [`realloc`](std::alloc::GlobalAlloc::realloc)
//! delegates to `std::alloc::System::realloc` — no allocate-copy-deallocate
//! emulation (unlike `peak_alloc` 0.3.0, whose realloc transiently spikes
//! `current` to `old + new`).
//!
//! # Usage with Criterion `iter_custom`
//!
//! ```rust,ignore
//! use shamir_bench_utils::peak_mem;
//!
//! // At the binary's crate root, before anything else:
//! shamir_bench_utils::install_peak_alloc!();
//!
//! // Inside iter_custom:
//! b.to_async(&rt).iter_custom(|iters| async move {
//!     let mut total_bytes: usize = 0;
//!     for _ in 0..iters {
//!         let (_result, peak) = peak_mem::measure(|| {
//!             // ... your workload ...
//!         });
//!         total_bytes += peak;
//!     }
//!     // Report as custom measurement or just track max.
//!     Duration::from_nanos(total_bytes as u64)
//! });
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Install the tracking allocator for THIS binary. Off by default: merely
/// linking `shamir_bench_utils` never changes the process allocator.
#[macro_export]
macro_rules! install_peak_alloc {
    () => {
        #[global_allocator]
        static __SHAMIR_BENCH_PEAK_ALLOC: $crate::peak_mem::PeakTracker =
            $crate::peak_mem::PeakTracker::new();
    };
}

/// Tracking allocator delegating every operation to [`System`], with
/// best-effort live/peak byte counters on top.
pub struct PeakTracker {
    system: System,
}

impl PeakTracker {
    /// Create the tracker (delegates all allocation to the system allocator).
    pub const fn new() -> Self {
        Self { system: System }
    }
}

impl Default for PeakTracker {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY (counters): telemetry is best-effort under concurrency — Relaxed
// ordering means the peak may lag by races, but every update is an atomic
// read-modify-write, so concurrent updates are never lost and `CURRENT`
// cannot be corrupted.
unsafe impl GlobalAlloc for PeakTracker {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe {
            let ptr = self.system.alloc(layout);
            if !ptr.is_null() {
                let cur = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
                PEAK.fetch_max(cur, Ordering::Relaxed);
            }
            ptr
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe {
            let ptr = self.system.alloc_zeroed(layout);
            if !ptr.is_null() {
                let cur = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
                PEAK.fetch_max(cur, Ordering::Relaxed);
            }
            ptr
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            self.system.dealloc(ptr, layout);
            CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        unsafe {
            let new_ptr = self.system.realloc(ptr, layout, new_size);
            if !new_ptr.is_null() {
                account_realloc(layout.size(), new_size);
            }
            new_ptr
        }
    }
}

/// Realloc accounting as atomic read-modify-write deltas: growth adds
/// `new - old` (peak takes `fetch_max`), shrink subtracts `old - new`. Never
/// an `old + new` spike, never a lost concurrent update.
pub(crate) fn account_realloc(old_size: usize, new_size: usize) {
    if new_size >= old_size {
        let grow = new_size - old_size;
        let cur = CURRENT.fetch_add(grow, Ordering::Relaxed) + grow;
        PEAK.fetch_max(cur, Ordering::Relaxed);
    } else {
        CURRENT.fetch_sub(old_size - new_size, Ordering::Relaxed);
    }
}

/// Reset the peak watermark to the current allocation level.
///
/// Call immediately before the workload you want to measure; subsequent
/// [`current_peak`] deltas are then relative to this point without needing
/// live allocations to be zero.
pub fn reset() {
    PEAK.store(CURRENT.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// Return the current peak memory (bytes) since the last [`reset`].
pub fn current_peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// Return the current total allocated bytes (not peak — live).
pub fn current_allocated() -> usize {
    CURRENT.load(Ordering::Relaxed)
}

/// Measure peak allocation of a synchronous closure.
///
/// Resets the peak counter, runs `f`, then captures the peak.
/// Returns `(result_of_f, peak_bytes)`.
///
/// # Example
///
/// ```rust,ignore
/// let (result, peak_bytes) = peak_mem::measure(|| {
///     let v: Vec<u8> = vec![0u8; 1024];
///     v.len()
/// });
/// assert!(peak_bytes >= 1024);
/// ```
pub fn measure<F, R>(f: F) -> (R, usize)
where
    F: FnOnce() -> R,
{
    reset();
    let r = f();
    let peak = current_peak();
    (r, peak)
}

/// Async version of [`measure`] — resets peak, awaits the future,
/// captures peak.
///
/// Note: because Rust's async tasks may interleave on the same thread,
/// the peak may include allocations from other tasks if the executor is
/// multi-threaded. For accurate per-task measurement, use a
/// single-threaded runtime or `current_thread` flavor.
pub async fn measure_async<F, R>(f: F) -> (R, usize)
where
    F: std::future::Future<Output = R>,
{
    reset();
    let r = f.await;
    let peak = current_peak();
    (r, peak)
}

#[cfg(test)]
mod tests;
