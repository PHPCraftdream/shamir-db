use crate::common::latency::{
    padding_for, target_constant_time_ms, LatencyPadGuard, FIXED_FLOOR_MS, JITTER_MAX_MS,
};
use std::time::Duration;

/// Spec §8.5: when elapsed is below the floor, padding fills the gap up
/// to the target.
#[test]
fn padding_fills_gap_below_target() {
    let pad = padding_for(Duration::from_millis(10), 50);
    assert_eq!(pad, Duration::from_millis(40));
}

/// Spec §8.5: when elapsed already exceeds the target, no extra sleep.
#[test]
fn padding_zero_when_elapsed_over_target() {
    let pad = padding_for(Duration::from_millis(80), 50);
    assert_eq!(pad, Duration::ZERO);
}

/// Spec §8.5: equal-to-target → zero padding.
#[test]
fn padding_zero_at_exact_target() {
    let pad = padding_for(Duration::from_millis(50), 50);
    assert_eq!(pad, Duration::ZERO);
}

/// Sampled target must always sit at or above the floor (spec §8.5).
#[test]
fn sampled_target_respects_floor() {
    for _ in 0..100 {
        assert!(target_constant_time_ms() >= FIXED_FLOOR_MS);
    }
}

/// Sampled target must never exceed FLOOR + JITTER_MAX.
#[test]
fn sampled_target_respects_max_jitter() {
    for _ in 0..100 {
        let t = target_constant_time_ms();
        assert!(t <= FIXED_FLOOR_MS + JITTER_MAX_MS);
    }
}

/// Diagram 01 step 14: jitter MUST cover the full \[0, JITTER_MAX\]
/// range. Previous buggy `floor.max(j) + j` form happened to satisfy
/// the floor and max-jitter bounds, but if `JITTER_MAX_MS` were ever
/// raised above `FIXED_FLOOR_MS` the result would jump non-linearly.
/// This test exercises the distribution: across many samples we expect
/// to see at least the boundaries 50 and 75.
#[test]
fn sampled_target_distribution_covers_floor_and_ceiling() {
    let mut saw_floor = false;
    let mut saw_ceiling = false;
    // 10000 samples: probability of missing either endpoint with
    // uniform[0,25] is (25/26)^10000 ≈ 0 — practically impossible.
    for _ in 0..10_000 {
        let t = target_constant_time_ms();
        if t == FIXED_FLOOR_MS {
            saw_floor = true;
        }
        if t == FIXED_FLOOR_MS + JITTER_MAX_MS {
            saw_ceiling = true;
        }
    }
    assert!(
        saw_floor,
        "must occasionally hit the FIXED_FLOOR_MS exactly"
    );
    assert!(
        saw_ceiling,
        "must occasionally hit FIXED_FLOOR_MS + JITTER_MAX_MS"
    );
}

/// LatencyPadGuard returns ~target after fast computation (under floor).
#[test]
fn guard_returns_positive_pad_for_fast_work() {
    let g = LatencyPadGuard::start();
    // Don't actually sleep — just compute. Elapsed ≈ µs.
    let pad = g.finish_with_target(50);
    assert!(pad > Duration::ZERO);
    assert!(pad <= Duration::from_millis(50));
}
