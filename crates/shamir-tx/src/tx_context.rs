//! Per-transaction state bundle.
//!
//! Created at tx begin, consumed at commit (by the executor), or
//! dropped at abort. Drop = RAII rollback: all staged state is lost,
//! no storage side-effects.

use bytes::Bytes;
use std::collections::HashMap;

use crate::staging_store::StagingStore;
use crate::types::{IsolationLevel, TxId};
use crate::IndexWriteOp;

/// Per-transaction state bundle.
///
/// Holds all mutable state accumulated during a transaction:
/// - **write_set** — per-table `StagingStore` buffers (set/remove ops).
/// - **index_write_set** — accumulated `IndexWriteOp`s across all tables.
/// - **tables_with_hnsw_staging** — which tables have HNSW vectors staged.
/// - **interner_overlay** — new `(key_name → id)` mappings for this tx.
/// - **counter_deltas** — per-table row-count adjustments.
/// - **read_set** — SSI read tracking `(table_id, key) → version_seen`.
///
/// Drop = RAII rollback: all staged state is simply lost, no I/O.
pub struct TxContext {
    /// Unique transaction identifier.
    pub tx_id: TxId,

    /// Interned repo identifier (from the engine's interner).
    pub repo_id: u64,

    /// MVCC snapshot version — reads see only committed versions
    /// ≤ this value.
    pub snapshot_version: u64,

    /// Requested isolation level.
    pub isolation: IsolationLevel,

    /// Per-table write staging. Key = table name (interned u64).
    /// Each `StagingStore` buffers set/remove ops for that table.
    pub write_set: HashMap<u64, StagingStore>,

    /// Accumulated index write ops across all tables. Applied
    /// atomically during commit (via `apply_index_ops`).
    pub index_write_set: Vec<IndexWriteOp>,

    /// Per-table HNSW staged vector info. Key = table name (interned).
    /// The actual staged vectors live inside `HnswAdapter::staged` — this
    /// field just tracks which tables have HNSW staging to commit/rollback.
    pub tables_with_hnsw_staging: Vec<u64>,

    /// Interner overlay: new `(key_name → id)` mappings created during
    /// this tx. Merged into base interner on commit; dropped on abort.
    pub interner_overlay: scc::HashMap<String, u64>,

    /// Per-table counter delta. Applied at commit:
    /// `counter.add(delta)` for each table.
    pub counter_deltas: HashMap<u64, i64>,

    /// SSI read-set: `(table_id, key) → version_seen`. Only populated
    /// when `isolation == Serializable`. Validated at commit:
    /// `current_version(key)` must equal `version_seen`, else abort.
    pub read_set: HashMap<(u64, Bytes), u64>,
}

impl TxContext {
    pub fn new(
        tx_id: TxId,
        repo_id: u64,
        snapshot_version: u64,
        isolation: IsolationLevel,
    ) -> Self {
        Self {
            tx_id,
            repo_id,
            snapshot_version,
            isolation,
            write_set: HashMap::new(),
            index_write_set: Vec::new(),
            tables_with_hnsw_staging: Vec::new(),
            interner_overlay: scc::HashMap::new(),
            counter_deltas: HashMap::new(),
            read_set: HashMap::new(),
        }
    }

    /// True if the tx has no pending writes / index ops / staging at all.
    pub fn is_empty(&self) -> bool {
        self.write_set.is_empty()
            && self.index_write_set.is_empty()
            && self.tables_with_hnsw_staging.is_empty()
            && self.interner_overlay.is_empty()
            && self.counter_deltas.is_empty()
    }

    /// Record a counter change for a table (e.g. +N for insert_many).
    pub fn bump_counter(&mut self, table_id: u64, delta: i64) {
        *self.counter_deltas.entry(table_id).or_insert(0) += delta;
    }

    /// Record a read for SSI validation (only if Serializable).
    pub fn record_read(&mut self, table_id: u64, key: Bytes, version: u64) {
        if self.isolation == IsolationLevel::Serializable {
            self.read_set.insert((table_id, key), version);
        }
    }

    /// Mark that a table has HNSW vectors staged under this tx.
    pub fn mark_hnsw_staging(&mut self, table_id: u64) {
        if !self.tables_with_hnsw_staging.contains(&table_id) {
            self.tables_with_hnsw_staging.push(table_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tx_context_is_empty() {
        let ctx = TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Snapshot);
        assert!(ctx.is_empty());
        assert_eq!(ctx.tx_id.raw(), 1);
        assert_eq!(ctx.snapshot_version, 10);
    }

    #[test]
    fn bump_counter_accumulates() {
        let mut ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
        ctx.bump_counter(1, 5);
        ctx.bump_counter(1, 3);
        ctx.bump_counter(2, -1);
        assert_eq!(ctx.counter_deltas[&1], 8);
        assert_eq!(ctx.counter_deltas[&2], -1);
    }

    #[test]
    fn record_read_only_for_serializable() {
        let mut ctx_si = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
        ctx_si.record_read(1, Bytes::from_static(b"k"), 5);
        assert!(ctx_si.read_set.is_empty(), "SI should not track reads");

        let mut ctx_ssi = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Serializable);
        ctx_ssi.record_read(1, Bytes::from_static(b"k"), 5);
        assert_eq!(ctx_ssi.read_set.len(), 1);
    }

    #[test]
    fn mark_hnsw_staging_deduplicates() {
        let mut ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
        ctx.mark_hnsw_staging(10);
        ctx.mark_hnsw_staging(10);
        ctx.mark_hnsw_staging(20);
        assert_eq!(ctx.tables_with_hnsw_staging.len(), 2);
    }

    #[test]
    fn is_empty_after_mutation() {
        let mut ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
        assert!(ctx.is_empty());
        ctx.bump_counter(1, 1);
        assert!(!ctx.is_empty());
    }

    #[test]
    fn drop_is_noop() {
        let ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
        drop(ctx);
    }
}
