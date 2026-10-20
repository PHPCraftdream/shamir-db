//! VIS-4c visibility-metric export: zero-touch registration and per-scrape
//! snapshot rendering for the acknowledgement/reconciliation telemetry that
//! lives on `RepoTxGate` (`shamir_tx::visibility_metrics`). Series naming
//! follows the neighbouring `shamir_tx_*` / `shamir_gate_*` conventions.
//! Cumulative totals are counters raised with `absolute` (overlapping scrapes
//! never regress them); the duration histograms are exported as cumulative
//! `*_bucket` gauges plus `*_count` counters. Labels carry only
//! mode/result/reason/kind plus the `db`/`repo` scope labels the gate
//! exporters already use — never a version or transaction id.

use shamir_tx::visibility_metrics::{
    kind_index, mode_index, reason_index, result_index, AckMode, AckResult, PendingKind,
    PendingReason, VisibilityMetricsSnapshot, BUCKET_BOUNDS_MS, BUCKET_COUNT,
};

/// `le` label for bucket `i` in Prometheus seconds notation; the last
/// bucket is `+Inf`.
fn le_label(i: usize) -> String {
    match BUCKET_BOUNDS_MS.get(i) {
        Some(&ms) => format!("{:.3}", ms as f64 / 1000.0),
        None => "+Inf".to_string(),
    }
}

const MODES: [AckMode; 2] = [AckMode::Strict, AckMode::Bounded];
const RESULTS: [AckResult; 4] = [
    AckResult::Visible,
    AckResult::PendingTimeout,
    AckResult::PendingReopen,
    AckResult::Error,
];
const REASONS: [PendingReason; 2] = [PendingReason::Timeout, PendingReason::ReopenRequired];
const KINDS: [PendingKind; 3] = [
    PendingKind::ExplicitTx,
    PendingKind::ImplicitBatch,
    PendingKind::DirectWrite,
];

/// Describe + zero-touch every series so the first scrape sees them all.
pub(crate) fn register_visibility_metrics() {
    metrics::describe_counter!(
        "shamir_visibility_ack_total",
        metrics::Unit::Count,
        "Acknowledgement outcomes by wait mode and result"
    );
    metrics::describe_counter!(
        "shamir_visibility_ack_wait_seconds_count",
        metrics::Unit::Count,
        "Acknowledgements whose wait duration was observed (never the \
         non-blocking fast path)"
    );
    metrics::describe_gauge!(
        "shamir_visibility_ack_wait_seconds_bucket",
        metrics::Unit::Count,
        "Cumulative acknowledgement waits at most `le` seconds"
    );
    metrics::describe_counter!(
        "shamir_visibility_pending_total",
        metrics::Unit::Count,
        "Pending visibility outcomes by reason and write kind"
    );
    metrics::describe_counter!(
        "shamir_visibility_read_wait_timeouts_total",
        metrics::Unit::Count,
        "AsOf reads that timed out waiting for version visibility"
    );
    metrics::describe_counter!(
        "shamir_visibility_reconciliation_total",
        metrics::Unit::Count,
        "WaitForVisibility reconciliation outcomes by result"
    );
    metrics::describe_counter!(
        "shamir_visibility_reconciliation_wait_seconds_count",
        metrics::Unit::Count,
        "Reconciliation waits whose duration was observed"
    );
    metrics::describe_gauge!(
        "shamir_visibility_reconciliation_wait_seconds_bucket",
        metrics::Unit::Count,
        "Cumulative reconciliation waits at most `le` seconds"
    );
    metrics::describe_gauge!(
        "shamir_visibility_ack_waiters",
        metrics::Unit::Count,
        "Acknowledgements currently parked on the completion tracker"
    );

    for mode in MODES {
        for result in RESULTS {
            metrics::counter!(
                "shamir_visibility_ack_total",
                "mode" => mode.label(), "result" => result.label()
            )
            .increment(0);
            metrics::counter!(
                "shamir_visibility_ack_wait_seconds_count",
                "mode" => mode.label(), "result" => result.label()
            )
            .increment(0);
            for bucket in 0..BUCKET_COUNT {
                metrics::gauge!(
                    "shamir_visibility_ack_wait_seconds_bucket",
                    "mode" => mode.label(), "result" => result.label(),
                    "le" => le_label(bucket)
                )
                .set(0.0);
            }
        }
    }
    for reason in REASONS {
        for kind in KINDS {
            metrics::counter!(
                "shamir_visibility_pending_total",
                "reason" => reason.label(), "kind" => kind.label()
            )
            .increment(0);
        }
    }
    metrics::counter!("shamir_visibility_read_wait_timeouts_total").increment(0);
    for result in RESULTS {
        metrics::counter!(
            "shamir_visibility_reconciliation_total",
            "result" => result.label()
        )
        .increment(0);
        metrics::counter!(
            "shamir_visibility_reconciliation_wait_seconds_count",
            "result" => result.label()
        )
        .increment(0);
        for bucket in 0..BUCKET_COUNT {
            metrics::gauge!(
                "shamir_visibility_reconciliation_wait_seconds_bucket",
                "result" => result.label(), "le" => le_label(bucket)
            )
            .set(0.0);
        }
    }
    metrics::gauge!("shamir_visibility_ack_waiters").set(0.0);
}

/// Cumulative bucket counts (Prometheus `le` semantics).
fn cumulative(buckets: &[u64; BUCKET_COUNT]) -> [u64; BUCKET_COUNT] {
    let mut out = [0_u64; BUCKET_COUNT];
    let mut running = 0_u64;
    for (out, &count) in out.iter_mut().zip(buckets.iter()) {
        running += count;
        *out = running;
    }
    out
}

/// Per-scrape rendering for one repository. `db`/`repo` scope labels match
/// the gate gauges in `observability.rs`. The waiters gauge reuses the
/// completion tracker's RAII registration — no second wait-path counter.
pub(crate) fn record_visibility_metrics(
    db: &str,
    repo: &str,
    snap: &VisibilityMetricsSnapshot,
    waiters: usize,
) {
    // Owned label values: the recorder keeps them beyond this call.
    let (db, repo) = (db.to_owned(), repo.to_owned());
    for mode in MODES {
        let m = mode_index(mode);
        for result in RESULTS {
            let r = result_index(result);
            metrics::counter!(
                "shamir_visibility_ack_total",
                "db" => db.clone(), "repo" => repo.clone(),
                "mode" => mode.label(), "result" => result.label()
            )
            .absolute(snap.ack_total[m][r]);
            let buckets = &snap.ack_wait_ms[m][r];
            metrics::counter!(
                "shamir_visibility_ack_wait_seconds_count",
                "db" => db.clone(), "repo" => repo.clone(),
                "mode" => mode.label(), "result" => result.label()
            )
            .absolute(buckets.iter().sum::<u64>());
            for (bucket, &count) in cumulative(buckets).iter().enumerate() {
                metrics::gauge!(
                    "shamir_visibility_ack_wait_seconds_bucket",
                    "db" => db.clone(), "repo" => repo.clone(),
                    "mode" => mode.label(), "result" => result.label(),
                    "le" => le_label(bucket)
                )
                .set(count as f64);
            }
        }
    }
    for reason in REASONS {
        let r = reason_index(reason);
        for kind in KINDS {
            metrics::counter!(
                "shamir_visibility_pending_total",
                "db" => db.clone(), "repo" => repo.clone(),
                "reason" => reason.label(), "kind" => kind.label()
            )
            .absolute(snap.pending_total[r][kind_index(kind)]);
        }
    }
    metrics::counter!(
        "shamir_visibility_read_wait_timeouts_total",
        "db" => db.clone(), "repo" => repo.clone()
    )
    .absolute(snap.read_wait_timeouts);
    for result in RESULTS {
        let r = result_index(result);
        metrics::counter!(
            "shamir_visibility_reconciliation_total",
            "db" => db.clone(), "repo" => repo.clone(), "result" => result.label()
        )
        .absolute(snap.reconciliation_total[r]);
        let buckets = &snap.reconciliation_wait_ms[r];
        metrics::counter!(
            "shamir_visibility_reconciliation_wait_seconds_count",
            "db" => db.clone(), "repo" => repo.clone(), "result" => result.label()
        )
        .absolute(buckets.iter().sum::<u64>());
        for (bucket, &count) in cumulative(buckets).iter().enumerate() {
            metrics::gauge!(
                "shamir_visibility_reconciliation_wait_seconds_bucket",
                "db" => db.clone(), "repo" => repo.clone(),
                "result" => result.label(), "le" => le_label(bucket)
            )
            .set(count as f64);
        }
    }
    metrics::gauge!("shamir_visibility_ack_waiters", "db" => db, "repo" => repo)
        .set(waiters as f64);
}
