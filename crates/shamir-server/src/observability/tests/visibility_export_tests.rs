//! VIS-4c: the exposition contains every new series from the first scrape.

use metrics_exporter_prometheus::PrometheusBuilder;
use shamir_tx::visibility_metrics::{
    mode_index, result_index, AckMode, AckResult, VisibilityMetricsSnapshot,
};

use super::super::visibility_export::{record_visibility_metrics, register_visibility_metrics};

const SERIES: [&str; 8] = [
    "shamir_visibility_ack_total",
    "shamir_visibility_ack_wait_seconds_bucket",
    "shamir_visibility_ack_wait_seconds_count",
    "shamir_visibility_pending_total",
    "shamir_visibility_read_wait_timeouts_total",
    "shamir_visibility_reconciliation_total",
    "shamir_visibility_reconciliation_wait_seconds_bucket",
    "shamir_visibility_ack_waiters",
];

// Red before VIS-4c: none of the shamir_visibility_* series existed in the
// exposition. Registration alone (zero-touch) must expose every series name.
#[test]
fn zero_touch_registration_exposes_every_series() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, register_visibility_metrics);
    let rendered = handle.render();
    for name in SERIES {
        assert!(rendered.contains(name), "zero-touch scrape missing {name}");
    }
    assert!(rendered.contains("mode=\"strict\""));
    assert!(rendered.contains("mode=\"bounded\""));
    assert!(rendered.contains("result=\"pending_timeout\""));
    assert!(rendered.contains("reason=\"reopen_required\""));
    assert!(rendered.contains("kind=\"implicit_batch\""));
    assert!(
        rendered.contains("le=\"0.001\""),
        "first bucket edge is 1 ms"
    );
    assert!(
        rendered.contains("le=\"30.000\""),
        "last finite edge is 30 s"
    );
    assert!(rendered.contains("le=\"+Inf\""));
}

// A recorded snapshot is rendered under the db/repo scope labels, and a
// repeated scrape of the same snapshot never double-counts.
#[test]
fn recorded_snapshot_carries_scope_labels_and_values() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut snap = VisibilityMetricsSnapshot::default();
    let (bounded, timeout) = (
        mode_index(AckMode::Bounded),
        result_index(AckResult::PendingTimeout),
    );
    snap.ack_total[bounded][timeout] = 3;
    snap.ack_wait_ms[bounded][timeout][0] = 2;
    snap.pending_total[0][2] = 5;
    snap.read_wait_timeouts = 4;
    snap.reconciliation_total[result_index(AckResult::Visible)] = 6;
    metrics::with_local_recorder(&recorder, || {
        record_visibility_metrics("ops_db", "ops_repo", &snap, 2);
        record_visibility_metrics("ops_db", "ops_repo", &snap, 2);
    });
    let rendered = handle.render();
    for name in SERIES {
        assert!(rendered.contains(name), "scrape missing {name}");
    }
    let ack_line = rendered
        .lines()
        .find(|line| {
            line.starts_with("shamir_visibility_ack_total{")
                && line.contains("db=\"ops_db\"")
                && line.contains("mode=\"bounded\"")
                && line.contains("result=\"pending_timeout\"")
        })
        .expect("labelled ack counter line");
    assert!(
        ack_line.ends_with(" 3"),
        "absolute counter, got: {ack_line}"
    );
    let pending_line = rendered
        .lines()
        .find(|line| {
            line.starts_with("shamir_visibility_pending_total{")
                && line.contains("repo=\"ops_repo\"")
                && line.contains("reason=\"timeout\"")
                && line.contains("kind=\"direct_write\"")
        })
        .expect("labelled pending counter line");
    assert!(pending_line.ends_with(" 5"), "got: {pending_line}");
    assert!(rendered.contains("shamir_visibility_ack_waiters{"));
}
