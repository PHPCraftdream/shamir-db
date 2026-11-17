use std::sync::atomic::{AtomicU64, Ordering};

use super::QueryMetrics;
use crate::repo::durability_policy::{Histogram, LATENCY_NS_BOUNDS};

/// Stable error labels. Unlisted and absent codes are recorded as `other`.
pub const ERROR_LABELS: [&str; 17] = [
    "write_committed_visibility_pending",
    "visibility_future_version",
    "visibility_wait_invalid",
    "as_of_visibility_timeout",
    "repo_requires_reopen",
    "as_of_future_version",
    "as_of_reopen_required",
    "table_migrating",
    "version_conflict",
    "tx_conflict",
    "index_drain_in_progress",
    "resync_required",
    "subscription_catalogue_invalid",
    "replication_destination_bound",
    "replication_destination_quarantined",
    "interner_quota_exceeded",
    "other",
];

/// Label used for absent or unrecognized error codes.
pub const ERROR_OTHER: &str = "other";
const ERROR_OTHER_INDEX: usize = ERROR_LABELS.len() - 1;

/// Per-repository lock-free query telemetry state.
pub struct EngineMetrics {
    durations: [Histogram; 5],
    errors: [AtomicU64; ERROR_LABELS.len()],
    rows: [AtomicU64; 2],
}

impl Default for EngineMetrics {
    fn default() -> Self {
        Self {
            durations: std::array::from_fn(|_| Histogram::new(LATENCY_NS_BOUNDS)),
            errors: std::array::from_fn(|_| AtomicU64::new(0)),
            rows: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl QueryMetrics for EngineMetrics {
    fn record_duration(&self, op: super::QueryOp, elapsed_ns: u64) {
        self.durations[op.index()].observe(elapsed_ns);
    }

    fn record_error(&self, code: Option<&str>) {
        let index = code
            .and_then(|code| {
                ERROR_LABELS[..ERROR_OTHER_INDEX]
                    .iter()
                    .position(|label| *label == code)
            })
            .unwrap_or(ERROR_OTHER_INDEX);
        self.errors[index].fetch_add(1, Ordering::Relaxed);
    }

    fn record_rows(&self, viewed: u64, returned: u64) {
        // Skip zero values entirely: no wasted atomic adds or exporter churn.
        if viewed == 0 && returned == 0 {
            return;
        }
        if viewed != 0 {
            self.rows[0].fetch_add(viewed, Ordering::Relaxed);
        }
        if returned != 0 {
            self.rows[1].fetch_add(returned, Ordering::Relaxed);
        }
    }
}

impl EngineMetrics {
    /// Copy metric state for export; allocations are confined to this cold path.
    pub fn snapshot(&self) -> super::EngineMetricsSnapshot {
        super::EngineMetricsSnapshot {
            durations: std::array::from_fn(|index| self.durations[index].snapshot()),
            errors: ERROR_LABELS
                .iter()
                .enumerate()
                .map(|(index, label)| (*label, self.errors[index].load(Ordering::Relaxed)))
                .collect(),
            rows_viewed: self.rows[0].load(Ordering::Relaxed),
            rows_returned: self.rows[1].load(Ordering::Relaxed),
        }
    }
}
