use shamir_db::engine::repo::instance::support::metrics::{
    EngineMetricsSnapshot, ERROR_LABELS, QUERY_OPS,
};

pub(crate) fn register_engine_metrics() {
    metrics::describe_gauge!(
        "shamir_query_duration_seconds_bucket",
        metrics::Unit::Count,
        "Cumulative query duration observations by operation and upper bound"
    );
    metrics::describe_counter!(
        "shamir_query_duration_seconds_count",
        metrics::Unit::Count,
        "Query duration observations by operation"
    );
    metrics::describe_gauge!(
        "shamir_query_duration_seconds_sum",
        metrics::Unit::Seconds,
        "Sum of query durations in seconds by operation"
    );
    metrics::describe_counter!(
        "shamir_query_errors_total",
        metrics::Unit::Count,
        "Query errors by stable error code"
    );
    metrics::describe_counter!(
        "shamir_query_rows_viewed_total",
        metrics::Unit::Count,
        "Rows viewed by queries"
    );
    metrics::describe_counter!(
        "shamir_query_rows_returned_total",
        metrics::Unit::Count,
        "Rows returned by queries"
    );
}

pub(crate) fn record_engine_metrics(db: &str, repo: &str, snapshot: &EngineMetricsSnapshot) {
    for (op, duration) in QUERY_OPS.into_iter().zip(&snapshot.durations) {
        if duration.torn {
            continue;
        }
        let mut cumulative = 0_u64;
        for (bound, count) in duration.bounds.iter().zip(&duration.buckets) {
            cumulative = cumulative.saturating_add(*count);
            metrics::gauge!(
                "shamir_query_duration_seconds_bucket",
                "db" => db.to_owned(),
                "repo" => repo.to_owned(),
                "op" => op.label(),
                "le" => (*bound as f64 / 1_000_000_000.0).to_string(),
            )
            .set(cumulative as f64);
        }
        if let Some(overflow) = duration.buckets.get(duration.bounds.len()) {
            cumulative = cumulative.saturating_add(*overflow);
        }
        metrics::gauge!(
            "shamir_query_duration_seconds_bucket",
            "db" => db.to_owned(),
            "repo" => repo.to_owned(),
            "op" => op.label(),
            "le" => "+Inf",
        )
        .set(cumulative as f64);
        metrics::counter!(
            "shamir_query_duration_seconds_count",
            "db" => db.to_owned(),
            "repo" => repo.to_owned(),
            "op" => op.label(),
        )
        .absolute(duration.count);
        metrics::gauge!(
            "shamir_query_duration_seconds_sum",
            "db" => db.to_owned(),
            "repo" => repo.to_owned(),
            "op" => op.label(),
        )
        .set(duration.sum as f64 / 1_000_000_000.0);
    }

    for (code, count) in &snapshot.errors {
        if ERROR_LABELS.contains(code) {
            metrics::counter!(
                "shamir_query_errors_total",
                "db" => db.to_owned(),
                "repo" => repo.to_owned(),
                "code" => *code,
            )
            .absolute(*count);
        }
    }
    metrics::counter!("shamir_query_rows_viewed_total", "db" => db.to_owned(), "repo" => repo.to_owned())
        .absolute(snapshot.rows_viewed);
    metrics::counter!("shamir_query_rows_returned_total", "db" => db.to_owned(), "repo" => repo.to_owned())
        .absolute(snapshot.rows_returned);
}
