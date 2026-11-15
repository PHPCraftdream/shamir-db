//! Bound mapping at every edge: value equal to a bound, one above, zero,
//! `u64::MAX` — inclusive `le` semantics (design §3.2).

use crate::Histogram;

/// Bucket vector after one observation of `v`.
fn buckets_after(v: u64) -> Vec<u64> {
    let h = Histogram::new(crate::LATENCY_NS_BOUNDS);
    h.observe(v);
    h.snapshot().buckets
}

#[test]
fn value_equal_to_bound_lands_in_that_bucket() {
    let mut expected = vec![0; crate::LATENCY_NS_BOUNDS.len() + 1];
    expected[3] = 1;
    assert_eq!(buckets_after(crate::LATENCY_NS_BOUNDS[3]), expected);
}

#[test]
fn value_one_above_bound_moves_to_next_bucket() {
    let mut expected = vec![0; crate::LATENCY_NS_BOUNDS.len() + 1];
    expected[4] = 1;
    assert_eq!(buckets_after(crate::LATENCY_NS_BOUNDS[3] + 1), expected);
}

#[test]
fn zero_maps_to_first_bucket() {
    let mut expected = vec![0; crate::LATENCY_NS_BOUNDS.len() + 1];
    expected[0] = 1;
    assert_eq!(buckets_after(0), expected);
}

#[test]
fn u64_max_maps_to_overflow_cell() {
    let mut expected = vec![0; crate::LATENCY_NS_BOUNDS.len() + 1];
    let last = expected.len() - 1;
    expected[last] = 1;
    assert_eq!(buckets_after(u64::MAX), expected);
}

#[test]
fn byte_bounds_map_inclusively() {
    let h = Histogram::new(crate::BYTES_BOUNDS);
    h.observe(4_096);
    h.observe(4_097);
    let snap = h.snapshot();
    assert_eq!(snap.buckets, vec![0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
}
