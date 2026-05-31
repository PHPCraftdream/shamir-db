use std::sync::atomic::{AtomicU64, Ordering};

/// Zero-dependency atomic counters for transaction telemetry.
///
/// Attach to `RepoTxGate` or keep standalone. All methods are lock-free.
#[derive(Default)]
pub struct TxMetrics {
    pub txs_started: AtomicU64,
    pub txs_committed: AtomicU64,
    pub txs_aborted_ssi: AtomicU64,
    pub txs_aborted_expired: AtomicU64,
    pub txs_aborted_storage: AtomicU64,
    pub txs_aborted_unique: AtomicU64,
    /// PROPOSED (Phase C). Committed tx aborted by Phase 2-bis because a
    /// concurrent committer wrote a key matching one of this tx's recorded
    /// predicate dependencies (a phantom). Disjoint from
    /// `txs_aborted_ssi` (point read-set) and `txs_aborted_unique`.
    pub txs_aborted_phantom: AtomicU64,
    /// Committed txs whose projections did NOT fully materialize inline on
    /// the commit path — recovery is the materialization guarantor on the
    /// next open (see `MaterializationState::Deferred`). NOT an abort.
    pub txs_materialization_deferred: AtomicU64,
    pub gc_runs: AtomicU64,
    pub gc_entries_deleted: AtomicU64,
}

impl TxMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_tx_start(&self) {
        self.txs_started.fetch_add(1, Ordering::Relaxed);
    }

    pub fn on_tx_committed(&self) {
        self.txs_committed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn on_tx_aborted_ssi(&self) {
        self.txs_aborted_ssi.fetch_add(1, Ordering::Relaxed);
    }

    pub fn on_tx_aborted_expired(&self) {
        self.txs_aborted_expired.fetch_add(1, Ordering::Relaxed);
    }

    pub fn on_tx_aborted_storage(&self) {
        self.txs_aborted_storage.fetch_add(1, Ordering::Relaxed);
    }

    pub fn on_tx_aborted_unique(&self) {
        self.txs_aborted_unique.fetch_add(1, Ordering::Relaxed);
    }

    pub fn on_tx_aborted_phantom(&self) {
        self.txs_aborted_phantom.fetch_add(1, Ordering::Relaxed);
    }

    /// A committed tx deferred at least one projection sub-phase to
    /// recovery (`MaterializationState::Deferred`). The tx is still
    /// COMMITTED — this counts deferral, not abort.
    pub fn on_tx_materialization_deferred(&self) {
        self.txs_materialization_deferred
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn on_gc_run(&self, entries_deleted: usize) {
        self.gc_runs.fetch_add(1, Ordering::Relaxed);
        self.gc_entries_deleted
            .fetch_add(entries_deleted as u64, Ordering::Relaxed);
    }

    /// Snapshot all counters for reporting.
    pub fn snapshot(&self) -> TxMetricsSnapshot {
        TxMetricsSnapshot {
            txs_started: self.txs_started.load(Ordering::Relaxed),
            txs_committed: self.txs_committed.load(Ordering::Relaxed),
            txs_aborted_ssi: self.txs_aborted_ssi.load(Ordering::Relaxed),
            txs_aborted_expired: self.txs_aborted_expired.load(Ordering::Relaxed),
            txs_aborted_storage: self.txs_aborted_storage.load(Ordering::Relaxed),
            txs_aborted_unique: self.txs_aborted_unique.load(Ordering::Relaxed),
            txs_aborted_phantom: self.txs_aborted_phantom.load(Ordering::Relaxed),
            txs_materialization_deferred: self.txs_materialization_deferred.load(Ordering::Relaxed),
            gc_runs: self.gc_runs.load(Ordering::Relaxed),
            gc_entries_deleted: self.gc_entries_deleted.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TxMetricsSnapshot {
    pub txs_started: u64,
    pub txs_committed: u64,
    pub txs_aborted_ssi: u64,
    pub txs_aborted_expired: u64,
    pub txs_aborted_storage: u64,
    pub txs_aborted_unique: u64,
    pub txs_aborted_phantom: u64,
    pub txs_materialization_deferred: u64,
    pub gc_runs: u64,
    pub gc_entries_deleted: u64,
}
