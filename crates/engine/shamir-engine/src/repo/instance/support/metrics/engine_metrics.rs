use std::sync::atomic::{AtomicU64, Ordering};

use super::{QueryMetrics, ShapeRegistry};
use crate::repo::durability_policy::{Histogram, HistogramSnapshot, LATENCY_NS_BOUNDS};
use shamir_engine_table::index_metrics::{IndexBuildObserver, IndexMetricKind};

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
    /// `shamir_index_build_seconds` — one observe per completed first-time
    /// index build, slot per [`IndexMetricKind`].
    index_builds: [Histogram; 3],
    /// `shamir_index_rebuild_seconds` — doctor/repair or identity-migration
    /// re-invocations of the create paths.
    index_rebuilds: [Histogram; 3],
    /// Bounded per-shape execution counters.
    shapes: ShapeRegistry,
}

impl Default for EngineMetrics {
    fn default() -> Self {
        Self {
            durations: std::array::from_fn(|_| Histogram::new(LATENCY_NS_BOUNDS)),
            errors: std::array::from_fn(|_| AtomicU64::new(0)),
            rows: std::array::from_fn(|_| AtomicU64::new(0)),
            index_builds: std::array::from_fn(|_| Histogram::new(LATENCY_NS_BOUNDS)),
            index_rebuilds: std::array::from_fn(|_| Histogram::new(LATENCY_NS_BOUNDS)),
            shapes: ShapeRegistry::default(),
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

    fn record_shape(&self, key: [u8; 16], elapsed_ns: u64, failed: bool) {
        self.shapes.record(key, elapsed_ns, failed);
    }
}

impl IndexBuildObserver for EngineMetrics {
    fn observe_index_build(&self, kind: IndexMetricKind, elapsed_ns: u64) {
        self.index_builds[kind.index()].observe(elapsed_ns);
    }

    fn observe_index_rebuild(&self, kind: IndexMetricKind, elapsed_ns: u64) {
        self.index_rebuilds[kind.index()].observe(elapsed_ns);
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
            shapes_tracked: self.shapes.tracked() as u64,
        }
    }

    /// Per-shape execution counters of this repository.
    pub fn shape_stats(&self) -> &ShapeRegistry {
        &self.shapes
    }

    /// Torn-free copies of the index build/rebuild histograms for the
    /// index/interner snapshot (scrape-time only).
    pub(crate) fn index_metric_parts(&self) -> ([HistogramSnapshot; 3], [HistogramSnapshot; 3]) {
        (
            std::array::from_fn(|index| self.index_builds[index].snapshot()),
            std::array::from_fn(|index| self.index_rebuilds[index].snapshot()),
        )
    }
}
