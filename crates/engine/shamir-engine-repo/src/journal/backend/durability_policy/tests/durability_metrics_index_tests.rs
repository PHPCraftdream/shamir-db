//! Index stability of `DurabilityMetricsSnapshot` (versioned by position):
//! legacy slots keep their documented meaning (`durability_export.rs` GAUGES
//! / COUNTERS order), the R4a PR-3 WAL rows land exactly at the public index
//! constants, and the trailing `fsync` payload is `None` without an
//! initialized WAL.

use std::sync::Arc;

use shamir_metrics::LATENCY_NS_BOUNDS;
use shamir_wal::{WalGroupCommit, WalSink};

use crate::durability_policy::{DurabilityMetrics, DurabilityMetricsSnapshot, DurabilityPolicy};

/// Legacy slots for a `Default` policy (buffered commit, durable serving)
/// with NO initialized WAL: tier flags 0, backend-mem 0, W/R and the derived
/// pending rows 0, pull observations 0; every counter 0; journal zeros; the
/// fsync payload absent.
#[test]
fn legacy_slots_hold_documented_values_without_a_wal() {
    let snapshot = DurabilityMetrics::default().snapshot(&DurabilityPolicy::default(), None);

    let gauges = [
        ("wal_effective_commit (Buffered)", 0),
        ("replication_serve (Durable)", 0),
        ("wal_backend_mem (no group)", 0),
        ("wal_synced_lag_versions (W-R)", 0),
        ("wal_watermark_w", 0),
        ("wal_recoverable_through", 0),
        ("replication_confirmed_bound", 0),
        ("replication_served_bound", 0),
        ("wal_pending_versions", 0),
        ("wal_first_pending_version", 0),
        ("wal_first_pending_age_ns", 0),
    ];
    for (idx, (documented, expected)) in gauges.iter().enumerate() {
        assert_eq!(
            snapshot.gauges[idx], *expected,
            "legacy gauge {idx} ({documented}) drifted"
        );
    }

    for (idx, value) in snapshot.counters.iter().enumerate().take(8) {
        assert_eq!(*value, 0, "legacy counter {idx} must stay 0 with no WAL");
    }
    assert!(snapshot.journal.iter().all(|v| *v == 0));
    assert!(
        snapshot.fsync.is_none(),
        "fsync payload must be None when the group is absent"
    );
}

/// The new slots land exactly at their constants. Over an initialized Mem
/// WAL (nothing appended) they read 0 — Mem reports 0 for the size/segment
/// rows, `shamir_wal_backend_mem` carries the backend signal — and the fsync
/// histogram is present, empty and untorn, with the latency bound set.
#[test]
fn new_slots_land_at_constants_with_an_initialized_mem_wal() {
    let group = WalGroupCommit::new(Arc::new(WalSink::mem()));
    let snapshot =
        DurabilityMetrics::default().snapshot(&DurabilityPolicy::default(), Some(&group));

    assert_eq!(snapshot.gauges[2], 1, "initialized Mem WAL observed");

    assert_eq!(
        snapshot.gauges[DurabilityMetricsSnapshot::GAUGE_WAL_SIZE_BYTES],
        0,
        "Mem reports 0 for the size row"
    );
    assert_eq!(
        snapshot.gauges[DurabilityMetricsSnapshot::GAUGE_WAL_SEGMENTS],
        0,
        "Mem reports 0 for the segment row"
    );
    assert_eq!(
        snapshot.gauges[DurabilityMetricsSnapshot::GAUGE_WAL_TRUNCATION_LAG_VERSIONS],
        0
    );
    assert_eq!(
        snapshot.gauges[DurabilityMetricsSnapshot::GAUGE_GROUP_COMMIT_WAITERS],
        0
    );
    assert_eq!(
        snapshot.counters[DurabilityMetricsSnapshot::COUNTER_WAL_BYTES_TOTAL],
        0
    );

    let fsync = snapshot
        .fsync
        .expect("initialized group must carry an fsync histogram");
    assert_eq!(fsync.count, 0);
    assert_eq!(fsync.sum, 0);
    assert!(!fsync.torn);
    assert_eq!(fsync.bounds, LATENCY_NS_BOUNDS);
    // Overflow cell: bounds.len() + 1 buckets, all empty on a fresh group.
    assert_eq!(fsync.buckets.len(), LATENCY_NS_BOUNDS.len() + 1);
    assert!(fsync.buckets.iter().all(|v| *v == 0));
}
