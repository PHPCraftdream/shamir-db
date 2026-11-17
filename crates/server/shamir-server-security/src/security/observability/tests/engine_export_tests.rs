use std::collections::BTreeSet;

use metrics_exporter_prometheus::PrometheusBuilder;
use shamir_db::engine::repo::instance::support::metrics::{
    EngineMetrics, EngineMetricsSnapshot, QueryMetrics, QueryOp, ERROR_LABELS, QUERY_OPS,
};

use super::super::export::engine::{record_engine_metrics, register_engine_metrics};

fn sample<'a>(rendered: &'a str, name: &str, labels: &[&str]) -> &'a str {
    rendered
        .lines()
        .find(|line| {
            let Some((metric, rest)) = line.split_once('{') else {
                return false;
            };
            metric == name && labels.iter().all(|label| rest.contains(label))
        })
        .unwrap_or_else(|| panic!("missing {name} with labels {labels:?}:\n{rendered}"))
}

fn assert_label_keys(line: &str, expected: &[&str]) {
    let labels = line
        .split_once('{')
        .expect("labelled series")
        .1
        .split_once('}')
        .expect("closed labels")
        .0;
    let actual: BTreeSet<_> = labels
        .split(',')
        .map(|label| label.split_once('=').expect("key=value label").0)
        .collect();
    let expected: BTreeSet<_> = expected.iter().copied().collect();
    assert_eq!(actual, expected, "unexpected labels: {line}");
}

#[test]
fn zero_touch_exposes_every_query_series_with_bounded_labels() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let snapshot = EngineMetrics::default().snapshot();
    metrics::with_local_recorder(&recorder, || {
        register_engine_metrics();
        record_engine_metrics("metrics_db", "metrics_repo", &snapshot);
    });
    let rendered = handle.render();
    let samples: Vec<_> = rendered
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect();
    assert_eq!(samples.len(), 5 * 22 + ERROR_LABELS.len() + 2, "{rendered}");

    for op in QUERY_OPS {
        let op_label = format!("op=\"{}\"", op.label());
        let labels = [
            "db=\"metrics_db\"",
            "repo=\"metrics_repo\"",
            op_label.as_str(),
        ];
        let buckets: Vec<_> = rendered
            .lines()
            .filter(|line| {
                line.starts_with("shamir_query_duration_seconds_bucket{")
                    && line.contains(&op_label)
            })
            .collect();
        assert_eq!(
            buckets.len(),
            20,
            "all 19 finite bounds and +Inf for {op_label}"
        );
        sample(
            &rendered,
            "shamir_query_duration_seconds_bucket",
            &[
                "db=\"metrics_db\"",
                "repo=\"metrics_repo\"",
                &op_label,
                "le=\"60\"",
            ],
        );
        for (metric, extra) in [
            ("shamir_query_duration_seconds_bucket", "le=\"+Inf\""),
            ("shamir_query_duration_seconds_count", "op="),
            ("shamir_query_duration_seconds_sum", "op="),
        ] {
            let mut required = labels.to_vec();
            required.push(extra);
            let line = sample(&rendered, metric, &required);
            let keys = if metric.ends_with("_bucket") {
                &["db", "repo", "op", "le"][..]
            } else {
                &["db", "repo", "op"][..]
            };
            assert_label_keys(line, keys);
        }
    }
    for code in ERROR_LABELS {
        let code_label = format!("code=\"{code}\"");
        assert_label_keys(
            sample(
                &rendered,
                "shamir_query_errors_total",
                &["db=\"metrics_db\"", "repo=\"metrics_repo\"", &code_label],
            ),
            &["db", "repo", "code"],
        );
    }
    for metric in [
        "shamir_query_rows_viewed_total",
        "shamir_query_rows_returned_total",
    ] {
        assert_label_keys(
            sample(
                &rendered,
                metric,
                &["db=\"metrics_db\"", "repo=\"metrics_repo\""],
            ),
            &["db", "repo"],
        );
    }
    for line in samples {
        assert!(
            !line.contains("table=") && !line.contains("function="),
            "{line}"
        );
    }
}

#[test]
fn observed_snapshots_are_absolute_seconds_and_torn_histograms_are_omitted() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let metrics = EngineMetrics::default();
    metrics.record_duration(QueryOp::Read, 125_000);
    metrics.record_duration(QueryOp::Read, 2_000_000);
    metrics.record_duration(QueryOp::Read, 61_000_000_000);
    metrics.record_duration(QueryOp::Write, 70_000_000);
    metrics.record_error(Some("version_conflict"));
    metrics.record_error(Some("not_in_allowlist"));
    metrics.record_rows(19, 7);
    let snapshot = metrics.snapshot();
    let mut torn: EngineMetricsSnapshot = EngineMetrics::default().snapshot();
    torn.durations[QueryOp::Read.index()].torn = true;

    metrics::with_local_recorder(&recorder, || {
        register_engine_metrics();
        record_engine_metrics("observed_db", "observed_repo", &snapshot);
        record_engine_metrics("observed_db", "observed_repo", &snapshot);
        record_engine_metrics("torn_only_db", "repo", &torn);
    });
    let rendered = handle.render();
    let count = sample(
        &rendered,
        "shamir_query_duration_seconds_count",
        &["db=\"observed_db\"", "op=\"read\""],
    );
    assert!(count.ends_with(" 3"), "absolute count: {count}");
    let sum = sample(
        &rendered,
        "shamir_query_duration_seconds_sum",
        &["db=\"observed_db\"", "op=\"read\""],
    );
    assert!(
        sum.ends_with(" 61.002125"),
        "ns converted to seconds: {sum}"
    );
    let write_count = sample(
        &rendered,
        "shamir_query_duration_seconds_count",
        &["db=\"observed_db\"", "op=\"write\""],
    );
    assert!(write_count.ends_with(" 1"), "write count: {write_count}");
    let write_sum = sample(
        &rendered,
        "shamir_query_duration_seconds_sum",
        &["db=\"observed_db\"", "op=\"write\""],
    );
    assert!(write_sum.ends_with(" 0.07"), "write duration: {write_sum}");
    let bucket = sample(
        &rendered,
        "shamir_query_duration_seconds_bucket",
        &["db=\"observed_db\"", "op=\"read\"", "le=\"0.00025\""],
    );
    assert!(bucket.ends_with(" 1"), "cumulative finite bucket: {bucket}");
    let last_finite = sample(
        &rendered,
        "shamir_query_duration_seconds_bucket",
        &["db=\"observed_db\"", "op=\"read\"", "le=\"60\""],
    );
    assert!(
        last_finite.ends_with(" 2"),
        "last finite bound includes observations through 60 s: {last_finite}"
    );
    let inf = sample(
        &rendered,
        "shamir_query_duration_seconds_bucket",
        &["db=\"observed_db\"", "op=\"read\"", "le=\"+Inf\""],
    );
    assert!(inf.ends_with(" 3"), "overflow-inclusive +Inf: {inf}");
    let other = sample(
        &rendered,
        "shamir_query_errors_total",
        &[
            "db=\"observed_db\"",
            "repo=\"observed_repo\"",
            "code=\"other\"",
        ],
    );
    assert!(
        other.ends_with(" 1"),
        "unknown code is allowlisted as other: {other}"
    );
    assert!(sample(
        &rendered,
        "shamir_query_errors_total",
        &[
            "db=\"observed_db\"",
            "repo=\"observed_repo\"",
            "code=\"version_conflict\"",
        ],
    )
    .ends_with(" 1"));
    assert!(sample(
        &rendered,
        "shamir_query_rows_viewed_total",
        &["db=\"observed_db\"", "repo=\"observed_repo\""],
    )
    .ends_with(" 19"));
    assert!(sample(
        &rendered,
        "shamir_query_rows_returned_total",
        &["db=\"observed_db\"", "repo=\"observed_repo\""],
    )
    .ends_with(" 7"));
    assert!(!rendered.lines().any(|line| {
        line.contains("shamir_query_duration_seconds_")
            && line.contains("db=\"torn_only_db\"")
            && line.contains("op=\"read\"")
    }));
}
