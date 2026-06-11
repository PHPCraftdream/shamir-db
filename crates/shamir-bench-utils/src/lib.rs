//! Bench helpers for the S.H.A.M.I.R. workspace.
//!
//! `BENCH_QUICK=1` collapses every group's `sample_size`,
//! `measurement_time`, and `warm_up_time` to the minimum criterion
//! accepts, so a full /opti baseline+after pair completes in seconds
//! instead of minutes.

use std::time::Duration;

use criterion::measurement::Measurement;
use criterion::BenchmarkGroup;

/// `true` when the `BENCH_QUICK` env-var is set to any of `1`/`true`/`yes`/`on`.
pub fn is_quick() -> bool {
    std::env::var("BENCH_QUICK")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Quick-mode sample-size floor. Criterion's minimum is 10.
pub fn sample_size(default: usize) -> usize {
    if is_quick() {
        10
    } else {
        default
    }
}

/// Quick-mode measurement-time floor.
pub fn measurement_time(default: Duration) -> Duration {
    if is_quick() {
        Duration::from_secs(1)
    } else {
        default
    }
}

/// Quick-mode warm-up-time floor.
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
