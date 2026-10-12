//! Plain-data snapshot of [`crate::visibility_metrics::VisibilityMetrics`].

use super::metrics::{
    ACK_MODES, ACK_RESULTS, BUCKET_COUNT, PENDING_KINDS, PENDING_REASONS, RECONCILIATION_RESULTS,
};

/// Read-only copy for exposition. Index order and label helpers live in the
/// metrics module (`mode_index`/`result_index`/`reason_index`/`kind_index`).
#[allow(clippy::type_complexity)] // fixed-shape label arrays, not dynamic data
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VisibilityMetricsSnapshot {
    pub ack_total: [[u64; ACK_RESULTS]; ACK_MODES],
    pub ack_wait_ms: [[[u64; BUCKET_COUNT]; ACK_RESULTS]; ACK_MODES],
    /// `[reason][kind]`.
    pub pending_total: [[u64; PENDING_KINDS]; PENDING_REASONS],
    pub read_wait_timeouts: u64,
    pub reconciliation_total: [u64; RECONCILIATION_RESULTS],
    pub reconciliation_wait_ms: [[u64; BUCKET_COUNT]; RECONCILIATION_RESULTS],
}
