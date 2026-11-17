use crate::repo::durability_policy::HistogramSnapshot;

/// Cold-path copy of per-repository query metrics for scrape exporters.
pub struct EngineMetricsSnapshot {
    pub durations: [HistogramSnapshot; 5],
    pub errors: Vec<(&'static str, u64)>,
    pub rows_viewed: u64,
    pub rows_returned: u64,
}
