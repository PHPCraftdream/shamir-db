//! Bench helpers for the S.H.A.M.I.R. workspace.
//!
//! **Quick mode is the default.** Every group's `sample_size`,
//! `measurement_time`, and `warm_up_time` collapse to the minimum
//! criterion accepts, so a full /opti baseline+after pair completes in
//! seconds instead of minutes.
//!
//! Set `BENCH_FULL=1` (or `true`/`yes`/`on`) for the slow, statistically-
//! rigorous run with the per-bench defaults — only useful for release-
//! signal capture, not iterative optimization.
//!
//! The legacy `BENCH_QUICK=1` env-var is still honored as a no-op
//! alias (mode is already quick by default), so old scripts don't break.

use std::time::Duration;

use criterion::measurement::Measurement;
use criterion::BenchmarkGroup;

/// `true` when the bench should run in QUICK mode.
///
/// Quick is the **default**; full-rigor mode is opt-in via
/// `BENCH_FULL=1` (or `true`/`yes`/`on`).
pub fn is_quick() -> bool {
    !is_full()
}

/// `true` when `BENCH_FULL` is set to any of `1`/`true`/`yes`/`on`.
///
/// Full mode disables the quick-mode floor and lets each bench use its
/// per-group defaults (typically sample_size=100, measurement=5s,
/// warm_up=3s). Use this for release-signal benchmarks; the cost is
/// minutes-per-bench vs seconds in quick mode.
pub fn is_full() -> bool {
    std::env::var("BENCH_FULL")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Sample-size floor. Quick mode (default) returns Criterion's minimum
/// of 10; full mode returns the `default`.
pub fn sample_size(default: usize) -> usize {
    if is_quick() {
        10
    } else {
        default
    }
}

/// Measurement-time floor. Quick mode (default) returns 1s; full mode
/// returns the `default`.
pub fn measurement_time(default: Duration) -> Duration {
    if is_quick() {
        Duration::from_secs(1)
    } else {
        default
    }
}

/// Warm-up-time floor. Quick mode (default) returns 1s; full mode
/// returns the `default`.
pub fn warm_up_time(default: Duration) -> Duration {
    if is_quick() {
        Duration::from_secs(1)
    } else {
        default
    }
}

/// Apply all three knobs at once.
pub fn tune<M: Measurement>(
    group: &mut BenchmarkGroup<'_, M>,
    sample_size_default: usize,
    measurement_secs: u64,
    warm_up_secs: u64,
) {
    group.sample_size(sample_size(sample_size_default));
    group.measurement_time(measurement_time(Duration::from_secs(measurement_secs)));
    group.warm_up_time(warm_up_time(Duration::from_secs(warm_up_secs)));
}
