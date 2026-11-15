//! Commit-write-log record types for the gate's SSI predicate checks.
use bytes::Bytes;
use shamir_collections::TFxMap;
use shamir_wal::{WalEntryV2, WalTicket};
use std::sync::Arc;

/// PROPOSED (Phase C). One captured commit's index-posting footprint for
/// the predicate-conflict check. Built engine-side from `TxContext` state
/// already collected at commit time (see `build_footprint_from_tx`).
///
/// Wire-shape mirror of the doc section 5.2 sketch. NOT durable: regenerated only
/// while a Serializable tx is alive — same lifetime discipline as
/// `MvccStore::version_cache` (mvcc_store.rs:446) and `active_snapshots`.
#[derive(Debug, Clone, Default)]
pub struct TableWriteFootprint {
    /// Set iff the tx wrote ANYTHING (data/index/counter) on this table.
    /// Drives the coarse `PredicateDep::TableScan` check.
    pub touched: bool,
    /// `SetPosting.key`s the tx staged for this table (already collected
    /// at `tx.index_write_set` — tx_context.rs:72 ; SetPosting variant at
    /// index_write_op.rs:15). `Bytes` is refcounted: this clone is cheap.
    /// Sorted ascending so future `binary_search` optimisations are free.
    pub inserted_index_keys: Vec<Bytes>,
}

/// PROPOSED (Phase C). One committed tx's write footprint, stored in the
/// commit-write log for predicate-conflict checks.
#[derive(Debug, Clone)]
pub struct CommitWriteRecord {
    pub commit_version: u64,
    /// Keyed by table_token (engine's `table_token()`).
    pub per_table: TFxMap<u64, TableWriteFootprint>,
}

impl CommitWriteRecord {
    pub fn is_empty(&self) -> bool {
        self.per_table.is_empty()
    }
}

/// Ticket and replay entry transferred to a resolver; taking it does not settle
/// either tracker. A tuple keeps this additive handover API directly usable.
pub type Unresolved = (Arc<WalTicket>, Arc<WalEntryV2>);
