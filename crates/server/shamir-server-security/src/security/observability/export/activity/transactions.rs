use shamir_tx::TxMetricsSnapshot;

pub(crate) fn register_transactions_metrics() {
    metrics::describe_counter!(
        "shamir_tx_aborted_unique_total",
        metrics::Unit::Count,
        "Transactions aborted by unique constraint conflicts"
    );
    metrics::describe_counter!(
        "shamir_tx_aborted_phantom_total",
        metrics::Unit::Count,
        "Transactions aborted by phantom conflicts"
    );
    metrics::describe_counter!(
        "shamir_tx_materialization_deferred_total",
        metrics::Unit::Count,
        "Committed transactions with deferred materialization"
    );
    metrics::describe_counter!(
        "shamir_tx_post_wal_orphans_total",
        metrics::Unit::Count,
        "Post-WAL orphaned transaction operations"
    );

    metrics::counter!("shamir_tx_aborted_unique_total").increment(0);
    metrics::counter!("shamir_tx_aborted_phantom_total").increment(0);
    metrics::counter!("shamir_tx_materialization_deferred_total").increment(0);
    metrics::counter!("shamir_tx_post_wal_orphans_total").increment(0);
}

pub(crate) fn record_transactions_metrics(snapshot: &TxMetricsSnapshot) {
    metrics::counter!("shamir_tx_aborted_unique_total").absolute(snapshot.txs_aborted_unique);
    metrics::counter!("shamir_tx_aborted_phantom_total").absolute(snapshot.txs_aborted_phantom);
    metrics::counter!("shamir_tx_materialization_deferred_total")
        .absolute(snapshot.txs_materialization_deferred);
    metrics::counter!("shamir_tx_post_wal_orphans_total").absolute(snapshot.post_wal_orphans_total);
}
