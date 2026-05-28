//! Per-transaction state bundle.
//!
//! Created at tx begin, consumed at commit (by the executor), or
//! dropped at abort. Drop = RAII rollback: all staged state is lost,
//! no storage side-effects.

use bytes::Bytes;
use std::collections::HashMap;

use crate::staging_store::StagingStore;
use crate::types::{IsolationLevel, TxId};
use crate::version_provider::VersionProvider;
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

    /// Accumulated index write ops across all tables, with per-op table
    /// attribution. Each entry is `(table_token, op)`. Applied atomically
    /// during commit (via `apply_index_ops`).
    pub index_write_set: Vec<(u64, IndexWriteOp)>,

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

    /// Token → original table name. Populated alongside `write_set`
    /// entries. Used at commit time to look up table names for WAL
    /// emission and interner merge (Stage 5).
    pub table_tokens: HashMap<u64, String>,

    /// Optional version provider for SSI read-set validation.
    /// When `None`, commit_tx Phase 2 falls back to a stub provider
    /// `|_, _| 0` that trivially passes — Snapshot and Serializable
    /// behave identically.
    pub version_provider: Option<std::sync::Arc<dyn VersionProvider>>,

    /// Wall-clock instant when this transaction was opened.
    /// Used for max-lifetime enforcement at commit time.
    pub started_at: std::time::Instant,
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
            table_tokens: HashMap::new(),
            version_provider: None,
            started_at: std::time::Instant::now(),
        }
    }

    /// How long this transaction has been open.
    pub fn elapsed(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    /// Whether this transaction has exceeded the given max lifetime.
    pub fn is_expired(&self, max_lifetime: std::time::Duration) -> bool {
        self.started_at.elapsed() > max_lifetime
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

    /// Validate the read-set against current committed versions.
    ///
    /// For Serializable Snapshot Isolation: every key the tx read must
    /// still be at the same version when we're about to commit. If any
    /// key has advanced, another tx wrote there → abort with the
    /// offending key.
    ///
    /// `version_provider(table_id, key) -> Option<u64>` is supplied by
    /// the caller. `None` = unknown table → conflict. `Some(0)` is the
    /// safe default for registered tables where the key has never been
    /// written (0 <= any version_seen → passes).
    pub fn validate_read_set<F>(&self, mut version_provider: F) -> Result<(), (u64, Bytes)>
    where
        F: FnMut(u64, &Bytes) -> Option<u64>,
    {
        for ((table_id, key), version_seen) in &self.read_set {
            match version_provider(*table_id, key) {
                None => return Err((*table_id, key.clone())),
                Some(current) if current > *version_seen => {
                    return Err((*table_id, key.clone()));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// Get-or-create a StagingStore for the given table token.
    ///
    /// Also records the human-readable table name in `table_tokens`
    /// so commit-time WAL emission can look it up.
    pub fn ensure_table_staging(
        &mut self,
        token: u64,
        name: &str,
        base: std::sync::Arc<dyn shamir_storage::types::Store>,
    ) -> &mut crate::staging_store::StagingStore {
        self.table_tokens
            .entry(token)
            .or_insert_with(|| name.to_string());
        self.write_set
            .entry(token)
            .or_insert_with(|| crate::staging_store::StagingStore::new(base))
    }

    /// Mark that a table has HNSW vectors staged under this tx.
    pub fn mark_hnsw_staging(&mut self, table_id: u64) {
        if !self.tables_with_hnsw_staging.contains(&table_id) {
            self.tables_with_hnsw_staging.push(table_id);
        }
    }

    /// Attach a version provider used by commit_tx Phase 2 for SSI
    /// validation. Returns `&mut Self` for builder-style chaining.
    pub fn set_version_provider(
        &mut self,
        provider: std::sync::Arc<dyn VersionProvider>,
    ) -> &mut Self {
        self.version_provider = Some(provider);
        self
    }

    /// Apply an overlay-id → base-id remap across all staged writes.
    ///
    /// Called during commit phase 1, immediately after
    /// `commit_interner_overlay`, so subsequent flush phases see
    /// stable base ids only.
    ///
    /// Errors if any staged value fails to decode/re-encode. Caller
    /// should abort the transaction on error.
    pub async fn apply_id_remap(
        &mut self,
        remap: &std::collections::HashMap<u64, u64>,
    ) -> Result<(), String> {
        if remap.is_empty() {
            return Ok(());
        }
        for staging in self.write_set.values() {
            staging
                .rewrite_set_bytes(|bytes| {
                    crate::id_remap::remap_inner_value_bytes(bytes.clone(), remap)
                        .map_err(|e| format!("remap encode: {e}"))
                })
                .await?;
        }
        Ok(())
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

    #[tokio::test]
    async fn apply_id_remap_rewrites_write_set_bytes() {
        use shamir_storage::storage_in_memory::InMemoryStore;
        use shamir_storage::types::Store;
        use shamir_types::core::interner::InternerKey;
        use shamir_types::types::common::TMap;
        use shamir_types::types::value::InnerValue;

        let mut tx = TxContext::new(
            crate::types::TxId::new(1),
            0,
            10,
            crate::types::IsolationLevel::Snapshot,
        );

        let base: std::sync::Arc<dyn Store> = std::sync::Arc::new(InMemoryStore::new());
        let staging = crate::staging_store::StagingStore::new(base);

        let mut m = TMap::default();
        m.insert(InternerKey::new(100), InnerValue::Str("v".into()));
        let val = InnerValue::Map(m);
        let key: shamir_storage::types::RecordKey = bytes::Bytes::from_static(b"k1");
        staging.set(key.clone(), val.to_bytes().unwrap()).await;

        tx.write_set.insert(7, staging);

        let mut remap = std::collections::HashMap::new();
        remap.insert(100u64, 1000u64);
        tx.apply_id_remap(&remap).await.unwrap();

        let bytes = tx.write_set[&7].get(key).await.unwrap();
        let decoded = InnerValue::from_bytes(&bytes).unwrap();
        if let InnerValue::Map(m) = decoded {
            assert!(
                m.get(&InternerKey::new(1000)).is_some(),
                "key 100 must have been remapped to 1000"
            );
            assert!(m.get(&InternerKey::new(100)).is_none());
        } else {
            panic!("expected Map");
        }
    }

    #[tokio::test]
    async fn apply_id_remap_empty_is_noop() {
        let mut tx = TxContext::new(
            crate::types::TxId::new(1),
            0,
            10,
            crate::types::IsolationLevel::Snapshot,
        );
        let empty = std::collections::HashMap::new();
        tx.apply_id_remap(&empty).await.unwrap();
    }

    #[test]
    fn validate_read_set_passes_when_versions_unchanged() {
        let mut tx = TxContext::new(
            crate::types::TxId::new(1),
            0,
            10,
            crate::types::IsolationLevel::Serializable,
        );
        tx.record_read(7, Bytes::from_static(b"k1"), 5);
        tx.record_read(7, Bytes::from_static(b"k2"), 8);

        // Provider returns same versions → no conflict.
        let result = tx.validate_read_set(|_t, k| match k.as_ref() {
            b"k1" => Some(5),
            b"k2" => Some(8),
            _ => Some(0),
        });
        assert!(result.is_ok());
    }

    #[test]
    fn validate_read_set_detects_advance() {
        let mut tx = TxContext::new(
            crate::types::TxId::new(2),
            0,
            10,
            crate::types::IsolationLevel::Serializable,
        );
        tx.record_read(7, Bytes::from_static(b"x"), 5);

        // Concurrent writer bumped version → conflict.
        let result =
            tx.validate_read_set(|_, k| if k.as_ref() == b"x" { Some(9) } else { Some(0) });
        assert!(result.is_err());
        let (table_id, key) = result.unwrap_err();
        assert_eq!(table_id, 7);
        assert_eq!(key, Bytes::from_static(b"x"));
    }

    #[test]
    fn validate_read_set_empty_passes() {
        let tx = TxContext::new(
            crate::types::TxId::new(3),
            0,
            10,
            crate::types::IsolationLevel::Serializable,
        );
        let result = tx.validate_read_set(|_, _| Some(99u64));
        assert!(result.is_ok(), "empty read_set must pass");
    }

    #[test]
    fn validate_read_set_zero_provider_always_passes_si_pattern() {
        let mut tx = TxContext::new(
            crate::types::TxId::new(4),
            0,
            10,
            crate::types::IsolationLevel::Serializable,
        );
        tx.record_read(7, Bytes::from_static(b"a"), 5);
        tx.record_read(7, Bytes::from_static(b"b"), 3);
        // Stub provider returns Some(0) — used by Stage 4.D.5 scaffold.
        // 0 <= any version_seen, so passes trivially.
        let result = tx.validate_read_set(|_, _| Some(0u64));
        assert!(result.is_ok());
    }

    #[test]
    fn ensure_table_staging_creates_new() {
        use shamir_storage::storage_in_memory::InMemoryStore;
        use shamir_storage::types::Store;

        let mut tx = TxContext::new(
            crate::types::TxId::new(1),
            0,
            10,
            crate::types::IsolationLevel::Snapshot,
        );
        let base: std::sync::Arc<dyn Store> = std::sync::Arc::new(InMemoryStore::new());
        let staging = tx.ensure_table_staging(42, "users", base);
        assert!(staging.is_empty());
        assert_eq!(tx.table_tokens.get(&42), Some(&"users".to_string()));
    }

    #[test]
    fn ensure_table_staging_returns_same() {
        use shamir_storage::storage_in_memory::InMemoryStore;
        use shamir_storage::types::Store;

        let mut tx = TxContext::new(
            crate::types::TxId::new(1),
            0,
            10,
            crate::types::IsolationLevel::Snapshot,
        );
        let base: std::sync::Arc<dyn Store> = std::sync::Arc::new(InMemoryStore::new());
        let _ = tx.ensure_table_staging(42, "users", base.clone());
        let s = tx.ensure_table_staging(42, "users", base);
        assert!(s.is_empty());
        assert_eq!(tx.write_set.len(), 1, "should reuse, not duplicate");
    }

    #[test]
    fn set_version_provider_attaches_to_tx() {
        use crate::version_provider::VersionProvider;

        struct MyProvider;
        impl VersionProvider for MyProvider {
            fn version_of(&self, _t: u64, _k: &bytes::Bytes) -> Option<u64> {
                Some(42)
            }
        }

        let mut tx = TxContext::new(
            crate::types::TxId::new(1),
            0,
            10,
            crate::types::IsolationLevel::Serializable,
        );
        assert!(tx.version_provider.is_none());

        tx.set_version_provider(std::sync::Arc::new(MyProvider));
        assert!(tx.version_provider.is_some());

        let v = tx
            .version_provider
            .as_ref()
            .unwrap()
            .version_of(0, &bytes::Bytes::from_static(b"k"));
        assert_eq!(v, Some(42));
    }

    #[test]
    fn validate_read_set_unknown_table_returns_conflict() {
        let mut tx = TxContext::new(
            crate::types::TxId::new(10),
            0,
            10,
            crate::types::IsolationLevel::Serializable,
        );
        tx.record_read(99, Bytes::from_static(b"key"), 5);

        // Provider returns None for table_id 99 → conflict.
        let result = tx.validate_read_set(|_, _| None);
        assert!(result.is_err());
        let (table_id, key) = result.unwrap_err();
        assert_eq!(table_id, 99);
        assert_eq!(key, Bytes::from_static(b"key"));
    }
}
