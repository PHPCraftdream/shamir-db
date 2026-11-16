use metrics_exporter_prometheus::PrometheusBuilder;
use shamir_db::engine::repo::durability_policy::{DurabilityMetricsSnapshot, HistogramSnapshot};

use super::super::export::{record_storage_metrics, register_storage_metrics};

/// Stand-in fsync bounds (nanoseconds): two sub-ms cells + the implicit
/// overflow cell, exact at the exporter's six-decimal `le` rendering.
const FSYNC_BOUNDS: &[u64] = &[100_000, 1_000_000];

fn fsync_snapshot(buckets: Vec<u64>, count: u64, sum_ns: u64, torn: bool) -> HistogramSnapshot {
    HistogramSnapshot {
        bounds: FSYNC_BOUNDS,
        buckets,
        count,
        sum: sum_ns,
        torn,
    }
}

#[test]
fn storage_export_renders_wal_rows_and_drops_torn_or_missing_fsync() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut full = DurabilityMetricsSnapshot::default();
    full.gauges[DurabilityMetricsSnapshot::GAUGE_WAL_SIZE_BYTES] = 4096;
    full.gauges[DurabilityMetricsSnapshot::GAUGE_WAL_SEGMENTS] = 3;
    full.gauges[DurabilityMetricsSnapshot::GAUGE_WAL_TRUNCATION_LAG_VERSIONS] = 9;
    full.gauges[DurabilityMetricsSnapshot::GAUGE_GROUP_COMMIT_WAITERS] = 2;
    full.counters[DurabilityMetricsSnapshot::COUNTER_WAL_BYTES_TOTAL] = 777;
    full.fsync = Some(fsync_snapshot(vec![1, 0, 2], 3, 1_500_000, false));
    let torn = DurabilityMetricsSnapshot {
        fsync: Some(fsync_snapshot(vec![5, 5, 5], 15, 9_999_999_999, true)),
        ..DurabilityMetricsSnapshot::default()
    };
    let none = DurabilityMetricsSnapshot::default();
    metrics::with_local_recorder(&recorder, || {
        register_storage_metrics();
        record_storage_metrics("db_a", "repo_a", &full);
        record_storage_metrics("db_a", "repo_a", &full);
        record_storage_metrics("db_b", "repo_b", &torn);
        record_storage_metrics("db_c", "repo_c", &none);
    });
    let rendered = handle.render();
    for (metric, kind) in [
        ("shamir_wal_size_bytes", "gauge"),
        ("shamir_wal_segments", "gauge"),
        ("shamir_wal_truncation_lag_versions", "gauge"),
        ("shamir_group_commit_waiters", "gauge"),
        ("shamir_wal_bytes_total", "counter"),
        ("shamir_wal_fsync_seconds_count", "counter"),
        ("shamir_wal_fsync_seconds_bucket", "gauge"),
        ("shamir_wal_fsync_seconds_sum", "gauge"),
    ] {
        assert!(
            rendered.contains(&format!("# TYPE {metric} {kind}")),
            "missing TYPE line for {metric}: {rendered}"
        );
    }
    let samples: Vec<_> = rendered
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect();
    assert!(!samples.is_empty());
    for line in &samples {
        assert!(line.contains("db=\""), "missing db: {line}");
        assert!(line.contains("repo=\""), "missing repo: {line}");
        let expected_eq = if line.starts_with("shamir_wal_fsync_seconds_bucket") {
            3
        } else {
            2
        };
        assert_eq!(
            line.matches('=').count(),
            expected_eq,
            "unexpected labels: {line}"
        );
    }
    for (metric, value) in [
        ("shamir_wal_size_bytes", "4096"),
        ("shamir_wal_segments", "3"),
        ("shamir_wal_truncation_lag_versions", "9"),
        ("shamir_group_commit_waiters", "2"),
        // `.absolute`: the double record above must not double-count.
        ("shamir_wal_bytes_total", "777"),
    ] {
        let line = samples
            .iter()
            .find(|line| line.starts_with(metric) && line.contains("db=\"db_a\""))
            .unwrap_or_else(|| panic!("missing sample for {metric}: {rendered}"));
        assert!(line.ends_with(&format!(" {value}")), "wrong value: {line}");
    }
    // Cumulative `le` buckets in bound order, `+Inf` last carrying the total:
    // cells [1, 0, 2] over bounds [100µs, 1ms] render 1, 1, 3.
    let bucket = |le: &str| -> String {
        samples
            .iter()
            .find(|line| {
                line.starts_with("shamir_wal_fsync_seconds_bucket{")
                    && line.contains("db=\"db_a\"")
                    && line.contains(&format!("le=\"{le}\""))
            })
            .unwrap_or_else(|| panic!("missing bucket le={le}: {rendered}"))
            .to_string()
    };
    assert!(bucket("0.000100").ends_with(" 1"));
    assert!(bucket("0.001000").ends_with(" 1"));
    assert!(bucket("+Inf").ends_with(" 3"));
    let count_line = samples
        .iter()
        .find(|line| {
            line.starts_with("shamir_wal_fsync_seconds_count{") && line.contains("db=\"db_a\"")
        })
        .unwrap_or_else(|| panic!("missing fsync count: {rendered}"));
    assert!(count_line.ends_with(" 3"), "wrong count: {count_line}");
    let sum_line = samples
        .iter()
        .find(|line| {
            line.starts_with("shamir_wal_fsync_seconds_sum{") && line.contains("db=\"db_a\"")
        })
        .unwrap_or_else(|| panic!("missing fsync sum: {rendered}"));
    let sum: f64 = sum_line
        .rsplit(' ')
        .next()
        .unwrap_or_else(|| panic!("no value on sum line: {sum_line}"))
        .parse()
        .expect("fsync sum renders as float");
    // 1_500_000 ns total rendered as 0.0015 s, not raw nanoseconds.
    assert!(
        (sum - 0.0015).abs() < 1e-9,
        "sum not rendered in seconds: {sum_line}"
    );
    for db in ["db_b", "db_c"] {
        assert!(
            !samples.iter().any(|line| {
                line.starts_with("shamir_wal_fsync_seconds")
                    && line.contains(&format!("db=\"{db}\""))
            }),
            "fsync series exported for {db}"
        );
    }
    // Torn drops only the histogram; plain WAL rows still render for db_b.
    let line = samples
        .iter()
        .find(|line| line.starts_with("shamir_wal_size_bytes{") && line.contains("db=\"db_b\""))
        .unwrap_or_else(|| panic!("db_b gauges dropped with torn fsync: {rendered}"));
    assert!(line.ends_with(" 0"), "wrong value: {line}");
}
