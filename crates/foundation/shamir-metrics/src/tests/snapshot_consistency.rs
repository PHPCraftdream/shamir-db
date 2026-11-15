//! Consistent snapshot after many observations.

use crate::Histogram;

#[test]
fn many_observations_snapshot_matches_expected_state() {
    let h = Histogram::new(crate::LATENCY_NS_BOUNDS);
    let values = [
        5_000u64,
        120_000,
        100_000,
        2_500_000,
        60_000_000_000,
        1_000_000,
    ];
    for v in values {
        h.observe(v);
    }
    let snap = h.snapshot();
    assert!(!snap.torn);
    assert_eq!(snap.count, values.len() as u64);
    assert_eq!(snap.sum, values.iter().sum::<u64>());
    let mut expected = vec![0u64; crate::LATENCY_NS_BOUNDS.len() + 1];
    expected[0] = 2; // 5_000, 100_000 (<= 100_000)
    expected[1] = 1; // 120_000 (<= 250_000)
    expected[3] = 1; // 1_000_000 (<= 1_000_000)
    expected[4] = 1; // 2_500_000 (<= 2_500_000)
    expected[18] = 1; // 60_000_000_000 == last bound (inclusive)
    assert_eq!(snap.buckets, expected);
}

#[test]
fn repeated_identical_observations_aggregate_in_one_cell() {
    let h = Histogram::new(crate::LATENCY_NS_BOUNDS);
    for _ in 0..1_000 {
        h.observe(300_000);
    }
    let snap = h.snapshot();
    assert_eq!(snap.count, 1_000);
    assert_eq!(snap.sum, 300_000_000);
    let mut expected = vec![0u64; crate::LATENCY_NS_BOUNDS.len() + 1];
    expected[2] = 1_000; // 300_000 <= 500_000
    assert_eq!(snap.buckets, expected);
}
