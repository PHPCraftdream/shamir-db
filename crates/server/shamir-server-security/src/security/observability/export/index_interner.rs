//! Index/interner exporter (R4a PR-4): renders the scrape-time
//! [`IndexMetricsSnapshot`] into the `shamir_index_*` / `shamir_interner_*` /
//! `shamir_export_tables_*` families, next to the PR-2 query exporter.

use shamir_db::engine::repo::instance::support::metrics::{
    index_metrics_snapshot, IndexMetricKind, IndexMetricsSnapshot,
};

pub(crate) fn register_index_interner_metrics() {
    metrics::describe_gauge!(
        "shamir_index_entries",
        metrics::Unit::Count,
        "Index entries by table and index (capped cardinality; remainder in table=\"(other)\")"
    );
    for (base, what) in [
        ("shamir_index_build_seconds", "first-time index build"),
        ("shamir_index_rebuild_seconds", "index rebuild"),
    ] {
        metrics::describe_gauge!(
            format!("{base}_bucket"),
            metrics::Unit::Count,
            format!("Cumulative {what} observations by kind and upper bound")
        );
        metrics::describe_counter!(
            format!("{base}_count"),
            metrics::Unit::Count,
            format!("{what} observations by kind")
        );
        metrics::describe_gauge!(
            format!("{base}_sum"),
            metrics::Unit::Seconds,
            format!("Sum of {what} durations in seconds by kind")
        );
    }
    metrics::describe_gauge!(
        "shamir_interner_names",
        metrics::Unit::Count,
        "Field-name interner size (Interner::len())"
    );
    metrics::describe_gauge!(
        "shamir_interner_high_water",
        metrics::Unit::Count,
        "Persisted interner id high-water mark"
    );
    metrics::describe_gauge!(
        "shamir_export_tables_truncated",
        metrics::Unit::Count,
        "1 when the per-table cardinality cap is applied to this repository's export"
    );
    metrics::describe_gauge!(
        "shamir_export_tables_known",
        metrics::Unit::Count,
        "Number of open tables known at scrape (before the cardinality cap)"
    );
}

/// Walk the open-repository registry and record one snapshot per repo.
/// Errors are skipped per-repository, as the gate/degraded-index poller
/// path does; closed repos/tables are invisible (open-registry only).
pub(crate) async fn sample_index_interner_metrics(db: &shamir_db::ShamirDb) {
    for db_name in db.list_dbs() {
        let Some(instance) = db.get_db(&db_name) else {
            continue;
        };
        for repo_name in instance.list_repos() {
            let Some(repo) = instance.get_repo(&repo_name) else {
                continue;
            };
            let Ok(snapshot) = index_metrics_snapshot(&repo).await else {
                continue;
            };
            record_index_interner_metrics(&db_name, &repo_name, &snapshot);
        }
    }
}

pub(crate) fn record_index_interner_metrics(db: &str, repo: &str, snapshot: &IndexMetricsSnapshot) {
    for (base, parts) in [
        ("shamir_index_build_seconds", &snapshot.build),
        ("shamir_index_rebuild_seconds", &snapshot.rebuild),
    ] {
        for (kind, histogram) in IndexMetricKind::ALL.iter().zip(parts) {
            if histogram.torn {
                continue;
            }
            let mut cumulative = 0_u64;
            for (bound, count) in histogram.bounds.iter().zip(&histogram.buckets) {
                cumulative = cumulative.saturating_add(*count);
                metrics::gauge!(
                    format!("{base}_bucket"),
                    "db" => db.to_owned(),
                    "repo" => repo.to_owned(),
                    "kind" => kind.label(),
                    "le" => (*bound as f64 / 1_000_000_000.0).to_string(),
                )
                .set(cumulative as f64);
            }
            if let Some(overflow) = histogram.buckets.get(histogram.bounds.len()) {
                cumulative = cumulative.saturating_add(*overflow);
            }
            metrics::gauge!(
                format!("{base}_bucket"),
                "db" => db.to_owned(),
                "repo" => repo.to_owned(),
                "kind" => kind.label(),
                "le" => "+Inf",
            )
            .set(cumulative as f64);
            metrics::counter!(
                format!("{base}_count"),
                "db" => db.to_owned(),
                "repo" => repo.to_owned(),
                "kind" => kind.label(),
            )
            .absolute(histogram.count);
            metrics::gauge!(
                format!("{base}_sum"),
                "db" => db.to_owned(),
                "repo" => repo.to_owned(),
                "kind" => kind.label(),
            )
            .set(histogram.sum as f64 / 1_000_000_000.0);
        }
    }

    for row in &snapshot.entries {
        metrics::gauge!(
            "shamir_index_entries",
            "db" => db.to_owned(),
            "repo" => repo.to_owned(),
            "table" => row.table.clone(),
            "index" => row.index.clone(),
        )
        .set(row.entries as f64);
    }
    metrics::gauge!(
        "shamir_index_entries",
        "db" => db.to_owned(),
        "repo" => repo.to_owned(),
        "table" => "(other)",
    )
    .set(snapshot.entries_other as f64);

    metrics::gauge!("shamir_interner_names", "db" => db.to_owned(), "repo" => repo.to_owned())
        .set(snapshot.interner_names as f64);
    metrics::gauge!(
        "shamir_interner_high_water",
        "db" => db.to_owned(),
        "repo" => repo.to_owned(),
    )
    .set(snapshot.interner_high_water as f64);
    metrics::gauge!(
        "shamir_export_tables_truncated",
        "db" => db.to_owned(),
        "repo" => repo.to_owned(),
    )
    .set(u8::from(snapshot.tables_truncated) as f64);
    metrics::gauge!("shamir_export_tables_known", "db" => db.to_owned(), "repo" => repo.to_owned())
        .set(snapshot.tables_known as f64);
}
