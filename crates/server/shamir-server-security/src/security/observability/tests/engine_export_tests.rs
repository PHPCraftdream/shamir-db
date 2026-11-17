use std::collections::BTreeSet;

use metrics_exporter_prometheus::PrometheusBuilder;
use shamir_db::engine::repo::durability_policy::HistogramSnapshot;
use shamir_db::engine::repo::instance::support::metrics::{
    index_metrics_snapshot, EngineMetrics, EngineMetricsSnapshot, IndexEntryRow, IndexMetricKind,
    IndexMetricsSnapshot, QueryMetrics, QueryOp, ERROR_LABELS, INDEX_INDEXES_CAP, QUERY_OPS,
};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_builder::{batch::Batch, ddl, write};
use shamir_types::types::common::new_map;

use super::super::export::engine::{record_engine_metrics, register_engine_metrics};
use super::super::export::index_interner::{
    record_index_interner_metrics, register_index_interner_metrics,
};

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

// Stand-in index bounds (nanoseconds): 100 µs + 1 ms + implicit overflow,
// mirroring the fsync test's exact-at-six-decimals shape.
const INDEX_BOUNDS: &[u64] = &[100_000, 1_000_000];

fn empty_histogram() -> HistogramSnapshot {
    HistogramSnapshot {
        bounds: INDEX_BOUNDS,
        buckets: vec![0, 0, 0],
        count: 0,
        sum: 0,
        torn: false,
    }
}

fn synthetic_snapshot() -> IndexMetricsSnapshot {
    let mut build = std::array::from_fn(|_| empty_histogram());
    build[IndexMetricKind::Regular.index()] = HistogramSnapshot {
        bounds: INDEX_BOUNDS,
        buckets: vec![1, 0, 2],
        count: 3,
        sum: 1_500_000,
        torn: false,
    };
    let mut rebuild = std::array::from_fn(|_| empty_histogram());
    rebuild[IndexMetricKind::Regular.index()].torn = true;
    IndexMetricsSnapshot {
        build,
        rebuild,
        entries: vec![IndexEntryRow {
            table: "users".to_owned(),
            index: "users_email_idx".to_owned(),
            kind: IndexMetricKind::Regular,
            entries: 42,
        }],
        entries_other: 7,
        interner_names: 11,
        interner_high_water: 13,
        tables_known: 65,
        tables_truncated: true,
    }
}

#[test]
fn index_interner_export_renders_every_series_with_other_and_truncation() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let snapshot = synthetic_snapshot();
    metrics::with_local_recorder(&recorder, || {
        register_index_interner_metrics();
        record_index_interner_metrics("index_db", "index_repo", &snapshot);
    });
    let rendered = handle.render();
    // build: 3 kinds x (3 buckets + count + sum); rebuild: the torn regular
    // histogram is skipped, so 2 kinds x (3 buckets + count + sum); plus
    // 1 named row + 1 (other) row + 2 interner gauges + 2 cardinality gauges.
    let samples: Vec<_> = rendered
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect();
    assert_eq!(samples.len(), 31, "{rendered}");

    let entries = sample(
        &rendered,
        "shamir_index_entries",
        &[
            "db=\"index_db\"",
            "repo=\"index_repo\"",
            "table=\"users\"",
            "index=\"users_email_idx\"",
        ],
    );
    assert!(entries.ends_with(" 42"), "named row value: {entries}");
    let other = sample(
        &rendered,
        "shamir_index_entries",
        &[
            "db=\"index_db\"",
            "repo=\"index_repo\"",
            "table=\"(other)\"",
        ],
    );
    assert!(other.ends_with(" 7"), "aggregated (other) row: {other}");
    let truncated = sample(
        &rendered,
        "shamir_export_tables_truncated",
        &["db=\"index_db\"", "repo=\"index_repo\""],
    );
    assert!(truncated.ends_with(" 1"), "truncation flag: {truncated}");
    let known = sample(
        &rendered,
        "shamir_export_tables_known",
        &["db=\"index_db\"", "repo=\"index_repo\""],
    );
    assert!(known.ends_with(" 65"), "known tables: {known}");
    let names = sample(
        &rendered,
        "shamir_interner_names",
        &["db=\"index_db\"", "repo=\"index_repo\""],
    );
    assert!(names.ends_with(" 11"), "interner names: {names}");
    let high_water = sample(
        &rendered,
        "shamir_interner_high_water",
        &["db=\"index_db\"", "repo=\"index_repo\""],
    );
    assert!(
        high_water.ends_with(" 13"),
        "interner high-water: {high_water}"
    );
    for kind in ["regular", "sorted", "fts"] {
        let kind_label = format!("kind=\"{kind}\"");
        for metric in [
            "shamir_index_build_seconds_bucket",
            "shamir_index_build_seconds_count",
            "shamir_index_build_seconds_sum",
            "shamir_index_rebuild_seconds_bucket",
            "shamir_index_rebuild_seconds_count",
            "shamir_index_rebuild_seconds_sum",
        ] {
            sample(
                &rendered,
                metric,
                &["db=\"index_db\"", "repo=\"index_repo\"", &kind_label],
            );
        }
    }
}

#[test]
fn index_histograms_render_cumulative_buckets_count_and_seconds_sum() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let snapshot = synthetic_snapshot();
    metrics::with_local_recorder(&recorder, || {
        register_index_interner_metrics();
        // Recorded twice: the count counter must stay absolute.
        record_index_interner_metrics("index_db", "index_repo", &snapshot);
        record_index_interner_metrics("index_db", "index_repo", &snapshot);
    });
    let rendered = handle.render();
    let bucket = |le: &str| -> String {
        rendered
            .lines()
            .find(|line| {
                line.starts_with("shamir_index_build_seconds_bucket{")
                    && line.contains("kind=\"regular\"")
                    && line.contains(&format!("le=\"{le}\""))
            })
            .unwrap_or_else(|| panic!("missing build bucket le={le}: {rendered}"))
            .to_string()
    };
    assert!(bucket("0.0001").ends_with(" 1"), "first bucket: {rendered}");
    assert!(
        bucket("0.001").ends_with(" 1"),
        "empty middle bucket stays cumulative: {rendered}"
    );
    assert!(
        bucket("+Inf").ends_with(" 3"),
        "+Inf carries the overflow: {rendered}"
    );
    let count = sample(
        &rendered,
        "shamir_index_build_seconds_count",
        &["db=\"index_db\"", "repo=\"index_repo\"", "kind=\"regular\""],
    );
    assert!(count.ends_with(" 3"), "absolute count: {count}");
    let sum = sample(
        &rendered,
        "shamir_index_build_seconds_sum",
        &["db=\"index_db\"", "repo=\"index_repo\"", "kind=\"regular\""],
    );
    assert!(sum.ends_with(" 0.0015"), "ns sum in seconds: {sum}");
    assert!(
        !rendered
            .lines()
            .any(|line| line.starts_with("shamir_index_rebuild_seconds_")
                && line.contains("kind=\"regular\"")),
        "torn rebuild histogram is omitted: {rendered}"
    );
}

#[test]
fn index_entries_labels_are_exact() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let snapshot = synthetic_snapshot();
    metrics::with_local_recorder(&recorder, || {
        register_index_interner_metrics();
        record_index_interner_metrics("index_db", "index_repo", &snapshot);
    });
    let rendered = handle.render();
    assert_label_keys(
        sample(
            &rendered,
            "shamir_index_entries",
            &["table=\"users\"", "index=\"users_email_idx\""],
        ),
        &["db", "repo", "table", "index"],
    );
    assert_label_keys(
        sample(&rendered, "shamir_index_entries", &["table=\"(other)\""]),
        &["db", "repo", "table"],
    );
    for metric in [
        "shamir_interner_names",
        "shamir_interner_high_water",
        "shamir_export_tables_truncated",
        "shamir_export_tables_known",
    ] {
        assert_label_keys(
            sample(&rendered, metric, &["db=\"index_db\""]),
            &["db", "repo"],
        );
    }
    let count_line = sample(
        &rendered,
        "shamir_index_build_seconds_count",
        &["kind=\"fts\""],
    );
    assert_label_keys(count_line, &["db", "repo", "kind"]);
}

// ---------------------------------------------------------------------------
// Value-vs-state tests (R4a PR-4 chunk 3): a real in-memory engine is booted,
// driven through the builder API only, and the scrape is asserted through the
// exporter path (index_metrics_snapshot -> record_index_interner_metrics)
// against a local Prometheus recorder — never by reading state structs.
// ---------------------------------------------------------------------------

/// Boot a real in-memory `ShamirDb` with a `testdb` database holding a
/// `main` repository (in-memory engine) that owns the given tables.
async fn boot_testdb(tables: Vec<TableConfig>) -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await.expect("create_db");
    let mut repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    for table in tables {
        repo_config = repo_config.add_table(table);
    }
    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Run one builder batch against `testdb` and fail the test on error.
async fn execute_batch(shamir: &ShamirDb, build: impl FnOnce(&mut Batch)) {
    let mut b = Batch::new();
    b.id(1);
    build(&mut b);
    shamir
        .execute(
            "testdb",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .expect("batch executes");
}

/// Scrape `testdb`/`main` through the exporter path: the real snapshot
/// builder + `record_index_interner_metrics` under a fresh local recorder.
async fn exporter_render(shamir: &ShamirDb) -> String {
    let instance = shamir.get_db("testdb").expect("db exists");
    let repo = instance.get_repo("main").expect("repo exists");
    let snapshot = index_metrics_snapshot(&repo).await.expect("snapshot");
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        register_index_interner_metrics();
        record_index_interner_metrics("testdb", "main", &snapshot);
    });
    handle.render()
}

fn numeric_suffix(line: &str) -> f64 {
    line.rsplit(' ')
        .next()
        .expect("sample carries a value")
        .parse()
        .expect("sample value is numeric")
}

/// Build an index on a real engine: exactly one completed build observation
/// must be exported as `shamir_index_build_seconds_count == 1` with a
/// positive wall-clock `_sum`.
#[tokio::test]
async fn index_build_exports_count_one_with_positive_sum() {
    let shamir = boot_testdb(vec![TableConfig::new("users")]).await;
    execute_batch(&shamir, |b| {
        b.create_index(
            "idx",
            ddl::create_index("email_idx", "users").field("email"),
        );
    })
    .await;
    let rendered = exporter_render(&shamir).await;
    let count = sample(
        &rendered,
        "shamir_index_build_seconds_count",
        &["db=\"testdb\"", "repo=\"main\"", "kind=\"regular\""],
    );
    assert!(count.ends_with(" 1"), "exactly one build: {rendered}");
    let sum = sample(
        &rendered,
        "shamir_index_build_seconds_sum",
        &["db=\"testdb\"", "repo=\"main\"", "kind=\"regular\""],
    );
    assert!(
        numeric_suffix(sum) > 0.0,
        "build observed a positive duration: {sum}"
    );
}

/// Doctor repair drops and recreates the index: the rebuild histogram gains
/// its first observation while the build histogram stays at one.
#[tokio::test]
async fn repair_exports_one_rebuild_without_new_build() {
    let shamir = boot_testdb(vec![TableConfig::new("users")]).await;
    execute_batch(&shamir, |b| {
        b.create_index(
            "idx",
            ddl::create_index("email_idx", "users").field("email"),
        );
    })
    .await;
    let table = shamir
        .get_table("testdb", "main", "users")
        .await
        .expect("table");
    table.repair().await.expect("repair");
    let rendered = exporter_render(&shamir).await;
    let rebuild = sample(
        &rendered,
        "shamir_index_rebuild_seconds_count",
        &["db=\"testdb\"", "repo=\"main\"", "kind=\"regular\""],
    );
    assert!(rebuild.ends_with(" 1"), "exactly one rebuild: {rendered}");
    let build = sample(
        &rendered,
        "shamir_index_build_seconds_count",
        &["db=\"testdb\"", "repo=\"main\"", "kind=\"regular\""],
    );
    assert!(build.ends_with(" 1"), "build count unchanged: {rendered}");
    let rebuild_sum = sample(
        &rendered,
        "shamir_index_rebuild_seconds_sum",
        &["db=\"testdb\"", "repo=\"main\"", "kind=\"regular\""],
    );
    assert!(
        numeric_suffix(rebuild_sum) > 0.0,
        "rebuild observed a positive duration: {rebuild_sum}"
    );
}

/// Five real rows: the live entry-count API surfaces as
/// `shamir_index_entries == 5` through the exporter path.
#[tokio::test]
async fn entries_gauge_equals_written_row_count() {
    let shamir = boot_testdb(vec![TableConfig::new("users")]).await;
    execute_batch(&shamir, |b| {
        b.create_index(
            "idx",
            ddl::create_index("email_idx", "users").field("email"),
        );
    })
    .await;
    execute_batch(&shamir, |b| {
        for i in 0..5 {
            let mut m = new_map();
            m.insert("email".to_string(), QueryValue::Str(format!("u{i}@x")));
            b.insert(
                format!("ins{i}"),
                write::insert("users").row(QueryValue::Map(m)),
            );
        }
    })
    .await;
    let rendered = exporter_render(&shamir).await;
    let entries = sample(
        &rendered,
        "shamir_index_entries",
        &[
            "db=\"testdb\"",
            "repo=\"main\"",
            "table=\"users\"",
            "index=\"email_idx\"",
        ],
    );
    assert!(entries.ends_with(" 5"), "five indexed rows: {rendered}");
}

/// 70 tables each with a regular index: only the 32 index rows with the
/// highest record count (tie-break ascending table name) keep per-table
/// labels; the remaining 38 aggregate into the single `table="(other)"`
/// row, and the companion gauges report the true table count.
#[tokio::test]
async fn table_cap_aggregates_into_other_with_companion_gauges() {
    let tables: Vec<TableConfig> = (0..70)
        .map(|i| TableConfig::new(format!("t{i:02}")))
        .collect();
    let shamir = boot_testdb(tables).await;
    // A batch is capped at 50 queries: create the indexes in chunks.
    for chunk in (0..70).collect::<Vec<_>>().chunks(35) {
        let chunk = chunk.to_vec();
        execute_batch(&shamir, |b| {
            for i in chunk {
                b.create_index(
                    format!("idx{i}"),
                    ddl::create_index(format!("idx_t{i:02}"), format!("t{i:02}")).field("k"),
                );
            }
        })
        .await;
    }
    let rendered = exporter_render(&shamir).await;
    let named: Vec<_> = rendered
        .lines()
        .filter(|line| {
            line.starts_with("shamir_index_entries{") && !line.contains("table=\"(other)\"")
        })
        .collect();
    assert_eq!(
        named.len(),
        INDEX_INDEXES_CAP,
        "index-label cap honored: {rendered}"
    );
    let known = sample(
        &rendered,
        "shamir_export_tables_known",
        &["db=\"testdb\"", "repo=\"main\""],
    );
    assert!(known.ends_with(" 70"), "all tables known: {rendered}");
    let truncated = sample(
        &rendered,
        "shamir_export_tables_truncated",
        &["db=\"testdb\"", "repo=\"main\""],
    );
    assert!(truncated.ends_with(" 1"), "truncation flagged: {rendered}");
    let other = sample(
        &rendered,
        "shamir_index_entries",
        &["db=\"testdb\"", "repo=\"main\"", "table=\"(other)\""],
    );
    assert!(
        other.ends_with(" 0"),
        "empty rows aggregate to zero: {other}"
    );
}

/// The interner gauges export the live repo-interner state: rendered values
/// equal `Interner::len()` and `InternerManager::persisted_high_water()`.
#[tokio::test]
async fn interner_gauges_match_repo_interner_state() {
    let shamir = boot_testdb(vec![TableConfig::new("users")]).await;
    execute_batch(&shamir, |b| {
        b.create_index(
            "idx",
            ddl::create_index("email_idx", "users").field("email"),
        );
    })
    .await;
    execute_batch(&shamir, |b| {
        let mut m = new_map();
        m.insert("email".to_string(), QueryValue::Str("a@x".into()));
        m.insert("note".to_string(), QueryValue::Str("first".into()));
        b.insert("ins1", write::insert("users").row(QueryValue::Map(m)));
    })
    .await;
    let instance = shamir.get_db("testdb").expect("db exists");
    let repo = instance.get_repo("main").expect("repo exists");
    let interner_manager = repo.repo_interner().await.expect("repo interner");
    let expected_names = interner_manager.get().await.expect("interner").len();
    let expected_high_water = interner_manager.persisted_high_water();
    let rendered = exporter_render(&shamir).await;
    let names = sample(
        &rendered,
        "shamir_interner_names",
        &["db=\"testdb\"", "repo=\"main\""],
    );
    assert!(
        names.ends_with(&format!(" {expected_names}")),
        "interner names match live len: {names}"
    );
    let high_water = sample(
        &rendered,
        "shamir_interner_high_water",
        &["db=\"testdb\"", "repo=\"main\""],
    );
    assert!(
        high_water.ends_with(&format!(" {expected_high_water}")),
        "high water matches persisted mark: {high_water}"
    );
}
