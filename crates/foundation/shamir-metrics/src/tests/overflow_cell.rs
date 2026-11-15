//! The implicit overflow cell: values above the last bound count in one
//! extra cell so `le = +Inf` renders correctly.

use crate::Histogram;

#[test]
fn just_above_last_bound_fills_only_the_overflow_cell() {
    let h = Histogram::new(crate::LATENCY_NS_BOUNDS);
    let last = *crate::LATENCY_NS_BOUNDS.last().expect("non-empty bounds");
    h.observe(last + 1);
    let snap = h.snapshot();
    assert_eq!(snap.bounds, crate::LATENCY_NS_BOUNDS);
    let overflow = snap.buckets.len() - 1;
    let mut expected = vec![0u64; snap.bounds.len() + 1];
    expected[overflow] = 1;
    assert_eq!(snap.buckets, expected);
    assert_eq!(snap.count, 1);
    assert_eq!(snap.sum, last + 1);
}

#[test]
fn values_at_the_last_bound_do_not_reach_the_overflow_cell() {
    let h = Histogram::new(crate::LATENCY_NS_BOUNDS);
    let last = *crate::LATENCY_NS_BOUNDS.last().expect("non-empty bounds");
    h.observe(last);
    h.observe(last + 1);
    let snap = h.snapshot();
    let overflow = snap.buckets.len() - 1;
    assert_eq!(snap.buckets[overflow], 1);
    assert_eq!(snap.buckets[overflow - 1], 1);
    assert_eq!(snap.count, 2);
    assert_eq!(snap.sum, 2 * last + 1);
}
