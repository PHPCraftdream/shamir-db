//! Repository-owned durability export. Only db/repo labels, never a global
//! last-observed sample attached to each repository. Counters use absolute
//! like visibility_export: overlapping scrapes do not double-count totals.

use shamir_db::engine::repo::durability_policy::DurabilityMetricsSnapshot;

const GAUGES: [(&str, &str); 11] = [
    (
        "shamir_wal_effective_commit",
        "Configured WAL tier: 0 buffered, 1 synced; Mem backend ignores it",
    ),
    (
        "shamir_replication_serve",
        "Effective serve mode: 0 durable",
    ),
    (
        "shamir_wal_backend_mem",
        "Initialized WAL is in-memory: 1 yes, 0 no/uninitialized",
    ),
    (
        "shamir_wal_synced_lag_versions",
        "Uncovered version span W - R, not sparse entry count",
    ),
    (
        "shamir_wal_watermark_w",
        "Sampled WAL-written maximum version W",
    ),
    (
        "shamir_wal_recoverable_through",
        "Sampled contiguous recoverability prefix R",
    ),
    (
        "shamir_replication_confirmed_bound",
        "Last pull pre-clamp bound min(confirmed, W)",
    ),
    (
        "shamir_replication_served_bound",
        "Last pull served bound min(confirmed, W, R)",
    ),
    (
        "shamir_wal_pending_versions",
        "Uncovered version span W - R, not sparse entry count",
    ),
    (
        "shamir_wal_first_pending_version",
        "First uncovered position R + 1; zero if none observed",
    ),
    (
        "shamir_wal_first_pending_age_ns",
        "Lower-bound age since first observation of uncovered position",
    ),
];

const COUNTERS: [(&str, &str); 7] = [
    (
        "shamir_wal_sync_now_calls",
        "Registered sync demands, including pending and already-covered calls",
    ),
    (
        "shamir_wal_sync_rounds",
        "Completed sync worker rounds on this WAL instance",
    ),
    (
        "shamir_wal_sync_coalesced",
        "Demands observing an active sync worker",
    ),
    (
        "shamir_wal_sync_errors",
        "Failed sync worker rounds on this WAL instance",
    ),
    (
        "shamir_wal_sync_duration_ns",
        "Total nanoseconds for snapshot replay and sink sync",
    ),
    (
        "shamir_replication_pull_held_back_ns",
        "Total pull wait nanoseconds, including cancellation",
    ),
    (
        "shamir_replication_pull_held_back_count",
        "Pulls held back on recoverability, including cancellation",
    ),
];

const JOURNAL_COUNTERS: [(&str, &str); 6] = [
    (
        "shamir_journal_checkpoint_failures_total",
        "Failed runtime clean-horizon checkpoints",
    ),
    (
        "shamir_journal_checkpoints_total",
        "Successful runtime clean-horizon checkpoints",
    ),
    (
        "shamir_journal_retention_overflow_total",
        "Retention measurements above configured cap",
    ),
    (
        "shamir_journal_retention_refusals_total",
        "Retention enforcement passes refusing writes",
    ),
    (
        "shamir_journal_retention_gaps_total",
        "Durable retention GAP records",
    ),
    (
        "shamir_journal_channel_dropped_total",
        "Journal events dropped on bounded-channel overflow",
    ),
];

/// Describe at recorder initialization. Zero-touch is per registered repo at
/// scrape, not an unlabelled process series. No stores/workers are initialized.
pub(crate) fn register_durability_metrics() {
    for (name, help) in GAUGES {
        let unit = if name.ends_with("_ns") {
            metrics::Unit::Nanoseconds
        } else {
            metrics::Unit::Count
        };
        metrics::describe_gauge!(name, unit, help);
    }
    for (name, help) in COUNTERS.into_iter().chain(JOURNAL_COUNTERS) {
        let unit = if name.ends_with("_ns") {
            metrics::Unit::Nanoseconds
        } else {
            metrics::Unit::Count
        };
        metrics::describe_counter!(name, unit, help);
    }
}

/// Like neighbouring visibility metrics, recorder owns the scope labels.
/// Reopening under the same labels resets source counters, not the recorder's
/// absolute maximum; lower totals stay hidden until exceeding that maximum.
pub(crate) fn record_durability_metrics(
    db: &str,
    repo: &str,
    snapshot: &DurabilityMetricsSnapshot,
) {
    let (db, repo) = (db.to_owned(), repo.to_owned());
    for ((name, _), value) in GAUGES.into_iter().zip(snapshot.gauges) {
        metrics::gauge!(name, "db" => db.clone(), "repo" => repo.clone()).set(value as f64);
    }
    for ((name, _), value) in COUNTERS.into_iter().zip(snapshot.counters) {
        metrics::counter!(name, "db" => db.clone(), "repo" => repo.clone()).absolute(value);
    }
    for ((name, _), value) in JOURNAL_COUNTERS.into_iter().zip(snapshot.journal) {
        metrics::counter!(name, "db" => db.clone(), "repo" => repo.clone()).absolute(value);
    }
}
