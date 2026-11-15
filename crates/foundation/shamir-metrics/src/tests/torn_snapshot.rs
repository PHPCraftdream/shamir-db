//! The `torn` path: a writer that has bumped `started` without yet bumping
//! `completed` must exhaust the bounded retries and return a best-effort copy
//! with `torn: true`, not block.

use crate::atomics::Ordering;
use crate::Histogram;

#[test]
fn writer_in_flight_exhausts_retries_and_reports_torn() {
    let h = Histogram::new(crate::LATENCY_NS_BOUNDS);
    h.observe(150_000);
    // Writer mid-update: `started` bumped, `completed` not yet — the state a
    // preempted writer leaves behind.
    h.started.fetch_add(1, Ordering::Relaxed);
    let snap = h.snapshot();
    assert!(snap.torn);
    // Best-effort copy still carries the pre-write state.
    assert_eq!(snap.count, 1);
    assert_eq!(snap.sum, 150_000);
}

#[test]
fn balanced_counters_snapshot_is_not_torn() {
    let h = Histogram::new(crate::LATENCY_NS_BOUNDS);
    let snap = h.snapshot();
    assert!(!snap.torn);
    assert_eq!(snap.count, 0);
}
