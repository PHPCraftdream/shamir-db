//! Visibility-acknowledgement metrics (VIS-4c): lock-free counters, a
//! fixed-bucket duration histogram of atomics, and a plain-data snapshot.
//! The waiters gauge intentionally has NO counter here — it reuses the
//! completion tracker's existing RAII registration
//! (`RepoTxGate::completion().waiter_count()`).

mod gate_access;
mod metrics;
mod snapshot;

#[cfg(test)]
mod tests;

pub use metrics::{
    ack_result_of, bucket_index, kind_index, mode_index, reason_index, result_index, AckMode,
    AckResult, DurationHistogram, PendingKind, PendingReason, VisibilityMetrics, ACK_MODES,
    ACK_RESULTS, BUCKET_BOUNDS_MS, BUCKET_COUNT, PENDING_KINDS, PENDING_REASONS,
    RECONCILIATION_RESULTS,
};
pub use snapshot::VisibilityMetricsSnapshot;
