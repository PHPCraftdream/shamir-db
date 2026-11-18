use metrics_exporter_prometheus::PrometheusBuilder;
use shamir_db::engine::repo::durability_policy::DurabilityMetricsSnapshot;
use shamir_tx::TxMetrics;

use super::super::durability_export::{record_durability_metrics, register_durability_metrics};
use super::super::export::{record_transactions_metrics, register_transactions_metrics};

#[test]
fn durability_export_uses_repo_labels_and_absolute_snapshots_without_double_counting() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut a = DurabilityMetricsSnapshot::default();
    a.counters[0] = 3;
    a.gauges[3] = 7;
    a.journal = [4, 5, 6, 7, 8, 9];
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
        ("shamir_journal_checkpoint_failures_total", "db_a", "4"),
        ("shamir_journal_checkpoints_total", "db_a", "5"),
        ("shamir_journal_retention_gaps_total", "db_a", "8"),
        ("shamir_journal_channel_dropped_total", "db_a", "9"),
        ("shamir_journal_retention_overflow_total", "db_b", "0"),
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

#[test]
fn transactions_export_renders_zero_touch_absolute_unlabelled_counters() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let snapshot = |unique, phantom, deferred, orphans| {
        let metrics = TxMetrics::new();
        metrics
            .txs_aborted_unique
            .store(unique, std::sync::atomic::Ordering::Relaxed);
        metrics
            .txs_aborted_phantom
            .store(phantom, std::sync::atomic::Ordering::Relaxed);
        metrics
            .txs_materialization_deferred
            .store(deferred, std::sync::atomic::Ordering::Relaxed);
        metrics
            .post_wal_orphans_total
            .store(orphans, std::sync::atomic::Ordering::Relaxed);
        metrics.snapshot()
    };
    let first = snapshot(3, 5, 7, 11);
    let older = snapshot(2, 4, 6, 10);
    metrics::with_local_recorder(&recorder, register_transactions_metrics);
    let registered = handle.render();
    for name in [
        "shamir_tx_aborted_unique_total",
        "shamir_tx_aborted_phantom_total",
        "shamir_tx_materialization_deferred_total",
        "shamir_tx_post_wal_orphans_total",
    ] {
        assert!(
            registered.lines().any(|line| line == format!("{name} 0")),
            "missing zero-touch {name}: {registered}"
        );
    }
    metrics::with_local_recorder(&recorder, || {
        record_transactions_metrics(&first);
        record_transactions_metrics(&first);
        record_transactions_metrics(&older);
    });

    let rendered = handle.render();
    let samples: Vec<_> = rendered
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect();
    assert_eq!(
        samples.len(),
        4,
        "unexpected transaction series: {rendered}"
    );
    for (name, expected) in [
        ("shamir_tx_aborted_unique_total", "3"),
        ("shamir_tx_aborted_phantom_total", "5"),
        ("shamir_tx_materialization_deferred_total", "7"),
        ("shamir_tx_post_wal_orphans_total", "11"),
    ] {
        assert!(
            rendered
                .lines()
                .any(|line| line == format!("# TYPE {name} counter")),
            "missing counter TYPE for {name}: {rendered}"
        );
        let line = samples
            .iter()
            .find(|line| line.starts_with(&format!("{name} ")))
            .unwrap_or_else(|| panic!("missing unlabelled counter {name}: {rendered}"));
        assert!(
            line.ends_with(&format!(" {expected}")),
            "wrong snapshot value for {name}: {line}"
        );
        assert!(!line.contains('{'), "unexpected labels: {line}");
    }
}
