//! WAL/storage metric export (design `engine-metrics.md` §2.4, §3.6):
//! scrape-time snapshot rendering with `db`/`repo` labels only. Gauges
//! `.set`; the cumulative bytes counter `.absolute` so overlapping scrapes
//! never regress; the fsync histogram renders per the visibility-family
//! pattern — cumulative `*_bucket` gauges with `le` labels plus a `_count`
//! counter — with internal nanosecond cells rendered in SECONDS to match the
//! family name. `torn` snapshots and an uninitialized WAL (`fsync == None`)
//! export no fsync series for that repo on that scrape (design §3.4).

use shamir_db::engine::repo::durability_policy::{DurabilityMetricsSnapshot, HistogramSnapshot};

/// `le` label for bucket `i` in seconds; the trailing cell is the overflow
/// (`+Inf`). Cells are nanoseconds — six decimals keep every
/// `LATENCY_NS_BOUNDS` value distinct after the ns→s conversion.
fn le_label(bounds: &[u64], i: usize) -> String {
    match bounds.get(i) {
        Some(&ns) => format!("{:.6}", ns as f64 / 1e9),
        None => "+Inf".to_string(),
    }
}

/// Describe at recorder initialization. Zero-touch is per registered repo at
/// scrape, not an unlabelled process series. No stores/workers are initialized.
pub(crate) fn register_storage_metrics() {
    metrics::describe_gauge!(
        "shamir_wal_size_bytes",
        metrics::Unit::Bytes,
        "Approximate WAL bytes on disk: active segment + sealed retained; Mem backend reports 0"
    );
    metrics::describe_gauge!(
        "shamir_wal_segments",
        metrics::Unit::Count,
        "Retained WAL segment files: sealed + one active; Mem backend reports 0"
    );
    metrics::describe_gauge!(
        "shamir_wal_truncation_lag_versions",
        metrics::Unit::Count,
        "Max committed version minus the oldest truncatable sealed segment's max version; \
         0 when nothing is truncatable"
    );
    metrics::describe_gauge!(
        "shamir_group_commit_waiters",
        metrics::Unit::Count,
        "Sync demanders parked on WAL fsync coverage"
    );
    metrics::describe_counter!(
        "shamir_wal_bytes_total",
        metrics::Unit::Bytes,
        "WAL payload bytes appended since the WAL directory was created"
    );
    metrics::describe_counter!(
        "shamir_wal_fsync_seconds_count",
        metrics::Unit::Count,
        "Sync-worker round spanning snapshot replay + sink fsync (same span as \
         shamir_wal_sync_duration_ns)"
    );
    metrics::describe_gauge!(
        "shamir_wal_fsync_seconds_bucket",
        metrics::Unit::Count,
        "Cumulative sync-worker rounds at most `le` seconds"
    );
    metrics::describe_gauge!(
        "shamir_wal_fsync_seconds_sum",
        metrics::Unit::Seconds,
        "Total sync-worker round seconds spanning snapshot replay + sink fsync (same span as \
         shamir_wal_sync_duration_ns)"
    );
}

/// Per-scrape rendering for one repository. Values are read only through the
/// snapshot's public index constants, never by bare position.
pub(crate) fn record_storage_metrics(db: &str, repo: &str, snapshot: &DurabilityMetricsSnapshot) {
    // Owned label values: the recorder keeps them beyond this call.
    let (db, repo) = (db.to_owned(), repo.to_owned());
    metrics::gauge!(
        "shamir_wal_size_bytes",
        "db" => db.clone(),
        "repo" => repo.clone(),
    )
    .set(snapshot.gauges[DurabilityMetricsSnapshot::GAUGE_WAL_SIZE_BYTES] as f64);
    metrics::gauge!(
        "shamir_wal_segments",
        "db" => db.clone(),
        "repo" => repo.clone(),
    )
    .set(snapshot.gauges[DurabilityMetricsSnapshot::GAUGE_WAL_SEGMENTS] as f64);
    metrics::gauge!(
        "shamir_wal_truncation_lag_versions",
        "db" => db.clone(),
        "repo" => repo.clone(),
    )
    .set(snapshot.gauges[DurabilityMetricsSnapshot::GAUGE_WAL_TRUNCATION_LAG_VERSIONS] as f64);
    metrics::gauge!(
        "shamir_group_commit_waiters",
        "db" => db.clone(),
        "repo" => repo.clone(),
    )
    .set(snapshot.gauges[DurabilityMetricsSnapshot::GAUGE_GROUP_COMMIT_WAITERS] as f64);
    metrics::counter!(
        "shamir_wal_bytes_total",
        "db" => db.clone(),
        "repo" => repo.clone(),
    )
    .absolute(snapshot.counters[DurabilityMetricsSnapshot::COUNTER_WAL_BYTES_TOTAL]);
    if let Some(fsync) = &snapshot.fsync {
        record_fsync_histogram(&db, &repo, fsync);
    }
}

/// Cumulative `*_bucket` gauges (`+Inf` last) + `_count` counter + `_sum`
/// gauge in seconds. A `torn` snapshot is dropped for the repo on this
/// scrape instead of blocking the exporter (design §3.4).
fn record_fsync_histogram(db: &str, repo: &str, fsync: &HistogramSnapshot) {
    if fsync.torn {
        return;
    }
    let mut cumulative = 0_u64;
    for (i, &count) in fsync.buckets.iter().enumerate() {
        cumulative += count;
        metrics::gauge!(
            "shamir_wal_fsync_seconds_bucket",
            "db" => db.to_owned(),
            "repo" => repo.to_owned(),
            "le" => le_label(fsync.bounds, i),
        )
        .set(cumulative as f64);
    }
    metrics::counter!(
        "shamir_wal_fsync_seconds_count",
        "db" => db.to_owned(),
        "repo" => repo.to_owned(),
    )
    .absolute(fsync.count);
    metrics::gauge!(
        "shamir_wal_fsync_seconds_sum",
        "db" => db.to_owned(),
        "repo" => repo.to_owned(),
    )
    .set(fsync.sum as f64 / 1e9);
}
