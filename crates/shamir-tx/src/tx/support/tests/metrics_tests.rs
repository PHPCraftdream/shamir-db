use crate::TxMetrics;

// Distinct totals expose counters wired to the wrong field or omitted from snapshots.
#[test]
fn tx_metrics_defaults_independent_increments_and_gc_snapshot() {
    let m = TxMetrics::new();
    let zero = m.snapshot();
    assert_eq!(
        (
            zero.txs_started,
            zero.txs_committed,
            zero.txs_aborted_ssi,
            zero.txs_aborted_expired,
            zero.txs_aborted_storage,
            zero.txs_aborted_unique,
            zero.txs_aborted_phantom,
            zero.txs_materialization_deferred,
            zero.post_wal_orphans_total,
            zero.gc_runs,
            zero.gc_entries_deleted
        ),
        (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
    );
    m.on_tx_start();
    for _ in 0..2 {
        m.on_tx_committed();
    }
    for _ in 0..3 {
        m.on_tx_aborted_ssi();
    }
    for _ in 0..4 {
        m.on_tx_aborted_expired();
    }
    for _ in 0..5 {
        m.on_tx_aborted_storage();
    }
    for _ in 0..6 {
        m.on_tx_aborted_unique();
    }
    for _ in 0..7 {
        m.on_tx_aborted_phantom();
    }
    for _ in 0..8 {
        m.on_tx_materialization_deferred();
    }
    for _ in 0..9 {
        m.on_post_wal_orphan();
    }
    m.on_gc_run(4);
    m.on_gc_run(7);
    let s = m.snapshot();
    assert_eq!(
        (
            s.txs_started,
            s.txs_committed,
            s.txs_aborted_ssi,
            s.txs_aborted_expired,
            s.txs_aborted_storage,
            s.txs_aborted_unique,
            s.txs_aborted_phantom,
            s.txs_materialization_deferred,
            s.post_wal_orphans_total,
            s.gc_runs,
            s.gc_entries_deleted
        ),
        (1, 2, 3, 4, 5, 6, 7, 8, 9, 2, 11)
    );
    assert_eq!(zero.txs_started, 0);
}
