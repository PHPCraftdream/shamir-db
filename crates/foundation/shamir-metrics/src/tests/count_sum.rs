//! `count` and `sum` bookkeeping: integer `sum` in the bucket unit.

use crate::Histogram;

#[test]
fn count_and_sum_track_observations() {
    let h = Histogram::new(crate::LATENCY_NS_BOUNDS);
    h.observe(150_000);
    h.observe(50_000);
    h.observe(3_000_000);
    let snap = h.snapshot();
    assert_eq!(snap.count, 3);
    assert_eq!(snap.sum, 3_200_000);
}

#[test]
fn empty_histogram_reports_zeroes() {
    let h = Histogram::new(crate::LATENCY_NS_BOUNDS);
    let snap = h.snapshot();
    assert_eq!(snap.count, 0);
    assert_eq!(snap.sum, 0);
    assert!(!snap.torn);
    assert_eq!(snap.buckets, vec![0u64; crate::LATENCY_NS_BOUNDS.len() + 1]);
}

#[test]
fn snapshot_reports_bucket_cell_counts_le_sum() {
    let h = Histogram::new(crate::LATENCY_NS_BOUNDS);
    h.observe(100_000);
    h.observe(150_000);
    h.observe(500_000);
    let snap = h.snapshot();
    assert_eq!(snap.buckets[0], 1); // 100_000 <= 100_000
    assert_eq!(snap.buckets[1], 1); // 150_000 <= 250_000
    assert_eq!(snap.buckets[2], 1); // 500_000 <= 500_000
    assert_eq!(snap.buckets.iter().sum::<u64>(), snap.count);
}
