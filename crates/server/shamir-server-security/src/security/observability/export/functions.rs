//! Function, compile-cache and admission metric export (design
//! `engine-metrics.md` §2.7). State lives in `shamir-wasm-host` global
//! mirrors; this module renders it per-scrape. Per-(db, repo) rows
//! materialise per recorded entry, like the per-repo durability family.

use shamir_db::engine::function::{
    admission_rejection_totals, call_metrics_snapshot, compile_cache_metrics, CallMetricsSnapshot,
    ADMISSION_REJECTION_REASONS,
};

/// `le` label for bucket `i` in Prometheus seconds notation; the implicit
/// overflow cell renders `+Inf`.
fn le_label(bounds: &[u64], i: usize) -> String {
    match bounds.get(i) {
        Some(&ns) => format!("{:.6}", ns as f64 / 1e9),
        None => "+Inf".to_string(),
    }
}

/// Describe + zero-touch the unlabelled series so the first scrape sees
/// them; per-(db, repo) families materialise at scrape per recorded entry.
pub(crate) fn register_functions_metrics() {
    metrics::describe_counter!(
        "shamir_compile_cache_hits_total",
        metrics::Unit::Count,
        "Compile-cache hits served to module compiles"
    );
    metrics::describe_counter!(
        "shamir_compile_cache_misses_total",
        metrics::Unit::Count,
        "Incremental-cache lookup misses and uncached compilation attempts"
    );
    metrics::describe_gauge!(
        "shamir_compile_cache_bytes",
        metrics::Unit::Bytes,
        "On-disk compile-cache bytes indexed"
    );
    metrics::describe_gauge!(
        "shamir_compile_cache_entries",
        metrics::Unit::Count,
        "On-disk compile-cache entries indexed"
    );
    metrics::describe_counter!(
        "shamir_module_admission_rejected_total",
        metrics::Unit::Count,
        "Module admission rejections by reason class"
    );
    metrics::describe_counter!(
        "shamir_function_fuel_consumed_total",
        metrics::Unit::Count,
        "Fuel units permanently consumed by function calls"
    );
    metrics::describe_gauge!(
        "shamir_function_duration_seconds_bucket",
        metrics::Unit::Count,
        "Cumulative function call durations at most `le` seconds"
    );
    metrics::describe_counter!(
        "shamir_function_duration_seconds_count",
        metrics::Unit::Count,
        "Function calls observed at depth 0"
    );
    metrics::describe_gauge!(
        "shamir_function_duration_seconds_sum",
        metrics::Unit::Seconds,
        "Total observed function call duration in seconds"
    );
    metrics::describe_gauge!(
        "shamir_function_call_depth_high_water",
        metrics::Unit::Count,
        "Highest nesting depth reached by a function call"
    );

    metrics::counter!("shamir_compile_cache_hits_total").increment(0);
    metrics::counter!("shamir_compile_cache_misses_total").increment(0);
    metrics::gauge!("shamir_compile_cache_bytes").set(0.0);
    metrics::gauge!("shamir_compile_cache_entries").set(0.0);
    for reason in ADMISSION_REJECTION_REASONS {
        metrics::counter!("shamir_module_admission_rejected_total", "reason" => reason)
            .increment(0);
    }
}

/// Per-scrape rendering from the wasm-host snapshot mirrors.
pub(crate) fn record_functions_metrics() {
    let cache = compile_cache_metrics();
    metrics::counter!("shamir_compile_cache_hits_total").absolute(cache.hits);
    metrics::counter!("shamir_compile_cache_misses_total").absolute(cache.misses);
    metrics::gauge!("shamir_compile_cache_bytes").set(cache.bytes as f64);
    metrics::gauge!("shamir_compile_cache_entries").set(cache.entries as f64);
    for (reason, total) in admission_rejection_totals() {
        metrics::counter!("shamir_module_admission_rejected_total", "reason" => reason)
            .absolute(total);
    }
    for entry in call_metrics_snapshot() {
        record_function_snapshot(&entry);
    }
}

/// Export one owner snapshot; torn duration data is omitted without losing
/// the independently sampled fuel and depth values.
pub(crate) fn record_function_snapshot(entry: &CallMetricsSnapshot) {
    let (db, repo) = (entry.db.clone(), entry.repo.clone());
    metrics::counter!(
        "shamir_function_fuel_consumed_total",
        "db" => db.clone(), "repo" => repo.clone()
    )
    .absolute(entry.fuel_total);
    metrics::gauge!(
        "shamir_function_call_depth_high_water",
        "db" => db.clone(), "repo" => repo.clone()
    )
    .set(entry.depth_high_water as f64);

    if entry.duration.torn {
        return;
    }
    metrics::counter!(
        "shamir_function_duration_seconds_count",
        "db" => db.clone(), "repo" => repo.clone()
    )
    .absolute(entry.duration.count);
    metrics::gauge!(
        "shamir_function_duration_seconds_sum",
        "db" => db.clone(), "repo" => repo.clone()
    )
    .set(entry.duration.sum as f64 / 1e9);
    let mut cumulative = 0u64;
    for (i, &cell) in entry.duration.buckets.iter().enumerate() {
        cumulative += cell;
        metrics::gauge!(
            "shamir_function_duration_seconds_bucket",
            "db" => db.clone(), "repo" => repo.clone(),
            "le" => le_label(entry.duration.bounds, i)
        )
        .set(cumulative as f64);
    }
}
