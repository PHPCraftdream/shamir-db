//! Peak memory allocation tracking for benchmarks.
//!
//! The tracking allocator is **NEVER installed by linking this module**.
//! A measuring binary opts in explicitly with [`install_peak_alloc!`] at
//! its crate root; every other binary that links this crate keeps its own
//! allocator untouched. [`realloc`](std::alloc::GlobalAlloc::realloc)
//! delegates to `std::alloc::System::realloc` — no allocate-copy-deallocate
//! emulation (unlike `peak_alloc` 0.3.0, whose realloc transiently spikes
//! `current` to `old + new`). The tracking code adds two SeqCst atomic
//! operations per successful allocation and one per deallocation.
//!
//! # Usage with bench-scale-tool
//!
//! Install the allocator at the binary crate root and measure one workload:
//!
//! ```rust,ignore
//! shamir_bench_utils::install_peak_alloc!();
//! let mut h = bench_scale_tool::Harness::new("peak_memory", env!("CARGO_MANIFEST_DIR"));
//! h.bench("workload", || {
//!     let measurement = shamir_bench_utils::peak_mem::measure(|| workload());
//!     std::hint::black_box(measurement);
//! });
//! h.run();
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static WINDOW_ACTIVE: AtomicUsize = AtomicUsize::new(0);

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

// SAFETY (counters): the operations delegate pointers unchanged to System.
// SeqCst atomics serialize live-byte updates and peak maxima; an allocation
// uses fetch_add then fetch_max, while deallocation uses one fetch_sub.
unsafe impl GlobalAlloc for PeakTracker {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe {
            let ptr = self.system.alloc(layout);
            if !ptr.is_null() {
                account_alloc(layout.size());
            }
            ptr
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe {
            let ptr = self.system.alloc_zeroed(layout);
            if !ptr.is_null() {
                account_alloc(layout.size());
            }
            ptr
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            self.system.dealloc(ptr, layout);
            account_dealloc(layout.size());
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
        account_alloc(new_size - old_size);
    } else {
        account_dealloc(old_size - new_size);
    }
}

fn account_alloc(size: usize) {
    PEAK.fetch_max(
        CURRENT.fetch_add(size, Ordering::SeqCst) + size,
        Ordering::SeqCst,
    );
}

fn account_dealloc(size: usize) {
    CURRENT.fetch_sub(size, Ordering::SeqCst);
}

fn reset_peak_owned() {
    PEAK.store(CURRENT.load(Ordering::SeqCst), Ordering::SeqCst);
    PEAK.fetch_max(CURRENT.load(Ordering::SeqCst), Ordering::SeqCst);
}

/// Reset the raw watermark when no measurement is active.
///
/// Raw reset is unsupported concurrently with allocation or measurement;
/// prefer [`measure`] or [`measure_async`] for an exclusive window.
/// Returns `false` while an owned window is active.
#[must_use = "reset is unsupported during an active measurement window"]
pub fn reset() -> bool {
    let Some(_window) = MeasurementWindow::acquire() else {
        return false;
    };
    reset_peak_owned();
    true
}

/// Return the raw peak watermark since the last [`reset`].
///
/// This unowned snapshot is unsupported concurrently with allocation or reset;
/// use [`measure`] or [`measure_async`] for an exclusive measurement.
pub fn current_peak() -> usize {
    PEAK.load(Ordering::SeqCst)
}

/// Return the current live allocated byte count.
pub fn current_allocated() -> usize {
    CURRENT.load(Ordering::SeqCst)
}

/// Measure a synchronous workload with an exclusively owned process window.
/// Nested/concurrent windows return `None` without running `f`; the token
/// releases on unwind.
pub fn measure<F, R>(f: F) -> Option<(R, usize)>
where
    F: FnOnce() -> R,
{
    let _window = MeasurementWindow::acquire()?;
    reset_peak_owned();
    let result = f();
    Some((result, current_peak()))
}

/// Async measurement owns a token rather than a lock guard across await.
/// Nested/concurrent windows return `None` without polling `f`. Cancellation
/// and unwinding release the window; other task allocations count.
pub async fn measure_async<F, R>(f: F) -> Option<(R, usize)>
where
    F: std::future::Future<Output = R>,
{
    let _window = MeasurementWindow::acquire()?;
    reset_peak_owned();
    let result = f.await;
    Some((result, current_peak()))
}

struct MeasurementWindow;

impl MeasurementWindow {
    fn acquire() -> Option<Self> {
        WINDOW_ACTIVE
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for MeasurementWindow {
    fn drop(&mut self) {
        WINDOW_ACTIVE.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
