//! Peak memory allocation tracking for benchmarks.
//!
//! Gated behind the `peak_mem` cargo feature — **off by default** so
//! normal `cargo bench` paths are unaffected. Enable with:
//!
//! ```toml
//! shamir-bench-utils = { path = "...", features = ["peak_mem"] }
//! ```
//!
//! # Usage with Criterion `iter_custom`
//!
//! ```rust,ignore
//! use shamir_bench_utils::peak_mem;
//!
//! // In bench harness setup (top of file, before criterion_main):
//! peak_mem::setup();
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

use peak_alloc::PeakAlloc;

/// Global allocator wrapper. Activated by calling [`setup`] once.
///
/// This replaces the default allocator for the entire process when the
/// `peak_mem` feature is enabled. The overhead is a single atomic add
/// per allocation — negligible for bench workloads.
#[global_allocator]
static PEAK_ALLOC: PeakAlloc = PeakAlloc;

/// Initialize the peak allocator tracking.
///
/// Call once at the top of your bench main (before `criterion_main!`
/// or equivalent). This is a no-op — the `#[global_allocator]` does
/// the real work — but serves as a documentation anchor and future
/// extensibility point.
pub fn setup() {
    // Force a reference so the linker doesn't strip the global allocator
    // in LTO builds.
    let _ = &PEAK_ALLOC;
}

/// Reset the peak counter to the current allocation level.
///
/// Call immediately before the workload you want to measure.
pub fn reset() {
    PEAK_ALLOC.reset_peak_usage();
}

/// Return the current peak memory (bytes) since the last [`reset`].
pub fn current_peak() -> usize {
    PEAK_ALLOC.peak_usage()
}

/// Return the current total allocated bytes (not peak — live).
pub fn current_allocated() -> usize {
    PEAK_ALLOC.current_usage()
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
