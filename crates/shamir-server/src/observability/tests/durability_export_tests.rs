use metrics_exporter_prometheus::PrometheusBuilder;
use shamir_db::engine::repo::durability_policy::DurabilityMetricsSnapshot;

use super::super::durability_export::{record_durability_metrics, register_durability_metrics};

#[test]
fn durability_export_has_only_repo_labels_and_does_not_mix_sources() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut a = DurabilityMetricsSnapshot::default();
    a.gauges[3] = 7;
    a.counters[0] = 3;
    let b = DurabilityMetricsSnapshot::default();
    metrics::with_local_recorder(&recorder, || {
        register_durability_metrics();
        record_durability_metrics("db_a", "same_repo", &a);
        record_durability_metrics("db_a", "same_repo", &a);
        record_durability_metrics("db_b", "same_repo", &b);
    });
    let rendered = handle.render();
    let samples: Vec<_> = rendered
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect();
    assert!(!samples.is_empty());
    for line in &samples {
        assert!(line.contains("db=\""), "missing db: {line}");
        assert!(line.contains("repo=\"same_repo\""), "missing repo: {line}");
        assert_eq!(line.matches('=').count(), 2, "unexpected labels: {line}");
    }
    for (metric, db, value) in [
        ("shamir_wal_synced_lag_versions", "db_a", "7"),
        ("shamir_wal_synced_lag_versions", "db_b", "0"),
        ("shamir_wal_sync_now_calls", "db_a", "3"),
        ("shamir_wal_sync_now_calls", "db_b", "0"),
    ] {
        let line = samples
            .iter()
            .find(|line| line.starts_with(metric) && line.contains(&format!("db=\"{db}\"")))
            .unwrap();
        assert!(
            line.ends_with(&format!(" {value}")),
            "wrong source or double-count: {line}"
        );
    }
    assert!(rendered.contains("shamir_wal_first_pending_age_ns{"));
}
