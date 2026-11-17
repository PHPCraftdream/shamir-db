use crate::repo::instance::support::metrics::{EngineMetrics, QueryMetrics, QueryOp, ERROR_OTHER};

#[test]
fn records_operation_durations_errors_and_rows() {
    let metrics = EngineMetrics::default();
    metrics.record_duration(QueryOp::Read, 125_000);
    metrics.record_duration(QueryOp::Batch, 2_500_000);
    metrics.record_duration(QueryOp::Function, 50_000);
    metrics.record_error(Some("version_conflict"));
    metrics.record_error(None);
    metrics.record_error(Some("not_in_allowlist"));
    metrics.record_rows(17, 8);

    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.durations[QueryOp::Read.index()].count, 1);
    assert_eq!(snapshot.durations[QueryOp::Read.index()].sum, 125_000);
    assert_eq!(snapshot.durations[QueryOp::Batch.index()].count, 1);
    assert_eq!(snapshot.durations[QueryOp::Batch.index()].sum, 2_500_000);
    assert_eq!(snapshot.durations[QueryOp::Function.index()].count, 1);
    assert_eq!(snapshot.durations[QueryOp::Function.index()].sum, 50_000);
    assert_eq!(snapshot.durations[QueryOp::Write.index()].count, 0);
    assert_eq!(snapshot.durations[QueryOp::Tx.index()].count, 0);
    assert_eq!(snapshot.errors[8], ("version_conflict", 1));
    assert_eq!(snapshot.errors.last(), Some(&(ERROR_OTHER, 2)));
    assert_eq!(snapshot.rows_viewed, 17);
    assert_eq!(snapshot.rows_returned, 8);
}
