use std::time::Duration;

use crate::mvcc_store::write_receipt::VisibilityPendingReason;
use crate::visibility_metrics::{
    bucket_index, kind_index, mode_index, result_index, AckMode, AckResult, PendingKind,
    VisibilityMetrics, BUCKET_BOUNDS_MS, BUCKET_COUNT,
};

// Red before VIS-4c: bucket_index and the label index helpers did not exist.
#[test]
fn bucket_edges_are_left_closed_and_overflow_is_last() {
    assert_eq!(bucket_index(0), 0);
    // Left-closed `<= bound`: exactly at a bound stays in that bucket.
    for (i, bound) in BUCKET_BOUNDS_MS.iter().enumerate() {
        assert_eq!(bucket_index(*bound), i, "bound {bound} must map to {i}");
        assert_eq!(
            bucket_index(bound + 1),
            i + 1,
            "just above bound {bound} must overflow to {i}",
        );
    }
    assert_eq!(bucket_index(u64::MAX), BUCKET_COUNT - 1);
}

// Red before VIS-4c: no snapshot arithmetic existed.
#[test]
fn record_ack_counts_fast_path_without_histogram_observation() {
    let m = VisibilityMetrics::new();
    // Strict/Visible fast path: one counter increment, NO duration sample.
    m.record_ack(AckMode::Strict, AckResult::Visible, false, Duration::ZERO);
    let snap = m.snapshot();
    let (s, v) = (
        mode_index(AckMode::Strict),
        result_index(AckResult::Visible),
    );
    assert_eq!(snap.ack_total[s][v], 1);
    assert_eq!(snap.ack_wait_ms[s][v].iter().sum::<u64>(), 0);
    // A waited bounded timeout: counter PLUS one histogram observation.
    m.record_ack(
        AckMode::Bounded,
        AckResult::PendingTimeout,
        true,
        Duration::from_millis(17),
    );
    let snap = m.snapshot();
    let (b, t) = (
        mode_index(AckMode::Bounded),
        result_index(AckResult::PendingTimeout),
    );
    assert_eq!(snap.ack_total[b][t], 1);
    assert_eq!(snap.ack_wait_ms[b][t].iter().sum::<u64>(), 1);
    let cumulative: u64 = snap.ack_wait_ms[b][t]
        .iter()
        .take(bucket_index(17) + 1)
        .sum();
    assert_eq!(cumulative, 1, "17ms must land at or below its bound");
    // Strict row untouched by the bounded observation.
    assert_eq!(snap.ack_total[s][t], 0);
    // Observation errors never record a duration even when waited.
    m.record_ack(
        AckMode::Bounded,
        AckResult::Error,
        true,
        Duration::from_millis(9),
    );
    let snap = m.snapshot();
    assert_eq!(
        snap.ack_wait_ms[b][result_index(AckResult::Error)]
            .iter()
            .sum::<u64>(),
        0
    );
}

// Red before VIS-4c: pending counters had no reason/kind matrix.
#[test]
fn pending_counters_distinguish_reason_and_kind() {
    let m = VisibilityMetrics::new();
    m.record_pending(VisibilityPendingReason::Timeout, PendingKind::DirectWrite);
    m.record_pending(
        VisibilityPendingReason::ReopenRequired,
        PendingKind::ImplicitBatch,
    );
    let snap = m.snapshot();
    assert_eq!(
        snap.pending_total[0][kind_index(PendingKind::DirectWrite)],
        1
    );
    assert_eq!(
        snap.pending_total[1][kind_index(PendingKind::ImplicitBatch)],
        1
    );
    assert_eq!(
        snap.pending_total[0][kind_index(PendingKind::ExplicitTx)],
        0
    );
}

// Red before VIS-4c: read-wait and reconciliation counters did not exist.
#[test]
fn read_wait_and_reconciliation_counters_snapshot() {
    let m = VisibilityMetrics::new();
    m.record_read_wait_timeout();
    m.record_reconciliation(AckResult::Visible, false, Duration::ZERO);
    m.record_reconciliation(AckResult::PendingTimeout, true, Duration::from_millis(3));
    let snap = m.snapshot();
    assert_eq!(snap.read_wait_timeouts, 1);
    assert_eq!(
        snap.reconciliation_total[result_index(AckResult::Visible)],
        1
    );
    assert_eq!(
        snap.reconciliation_total[result_index(AckResult::PendingTimeout)],
        1
    );
    assert_eq!(
        snap.reconciliation_wait_ms[result_index(AckResult::PendingTimeout)]
            .iter()
            .sum::<u64>(),
        1
    );
}

// The reservation rate-limits like the gate's own 5 s visibility warning.
#[test]
fn pending_warning_reservation_limits_the_second_call() {
    let m = VisibilityMetrics::new();
    assert!(m.reserve_pending_warning());
    assert!(!m.reserve_pending_warning());
}
