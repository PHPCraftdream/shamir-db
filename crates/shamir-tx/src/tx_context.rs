//! Per-transaction state bundle.
//!
//! Created at tx begin, consumed at commit (by the executor), or
//! dropped at abort. Drop = RAII rollback: all staged state is lost,
//! no storage side-effects.

use bytes::Bytes;
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

use shamir_types::types::record_id::RecordId;

use crate::staging_store::StagingStore;
use crate::types::{IsolationLevel, TxId};
use crate::version_provider::VersionProvider;
use crate::IndexWriteOp;

/// A promise this tx makes about a unique-index posting: at stage time
/// the deterministic unique key `index_key` was free (or owned by this
/// tx's `owner`). Re-validated under `commit_lock` (closes the
/// tx-concurrent unique-violation hole) — two concurrent txs claiming
/// the same value produce the BYTE-IDENTICAL `index_key`, so a single
/// `info_store.get(index_key)` settles ownership decisively.
///
/// Layer note: `index_key` is built engine-side
/// (`build_index_key(true, name, values).to_bytes()`) and handed in as
/// raw `Bytes`; shamir-tx stays ignorant of how the key is composed.
#[derive(Debug, Clone)]
pub struct UniqueGuard {
    /// Owning table token (engine `table_token()`), used to resolve the
    /// table's `info_store` at commit time.
    pub table_token: u64,
    /// The deterministic 25-byte unique-index key this tx intends to own.
    pub index_key: Bytes,
    /// The rid claiming the value. An update re-writing its own value is
    /// not a self-conflict (`existing == owner` → OK).
    pub owner: RecordId,
}

/// Per-transaction state bundle.
///
/// Holds all mutable state accumulated during a transaction:
/// - **write_set** — per-table `StagingStore` buffers (set/remove ops).
/// - **index_write_set** — accumulated `IndexWriteOp`s across all tables.
/// - **staged_vectors** — per-table HNSW vectors awaiting commit.
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

    /// Per-table HNSW staged vectors. Key = table token (interned table
    /// name). Each entry is a `(RecordId, embedding)` pair routed here by
    /// the executor instead of into the live HNSW graph. Promoted into the
    /// graph atomically at commit (Phase 5d); discarded by RAII drop on
    /// abort — exactly like every other tx-local field. This is the home
    /// for vector staging: nothing lives outside the `TxContext` anymore.
    pub staged_vectors: HashMap<u64, Vec<(RecordId, Vec<f32>)>>,

    /// Interner overlay: new `(key_name → id)` mappings created during
    /// this tx. Merged into base interner on commit; dropped on abort.
    pub interner_overlay: scc::HashMap<String, u64>,

    /// Next id to hand out from the overlay.  Starts at
    /// [`OVERLAY_ID_BASE`](crate::layered_interner::OVERLAY_ID_BASE)
    /// so overlay ids never clash with base ids.
    pub next_overlay_id: AtomicU64,

    /// Per-table counter delta. Applied at commit:
    /// `counter.add(delta)` for each table.
    pub counter_deltas: HashMap<u64, i64>,

    /// SSI read-set: `(table_id, key) → version_seen`. Only populated
    /// when `isolation == Serializable`. Validated at commit:
    /// `current_version(key)` must equal `version_seen`, else abort.
    ///
    /// `scc::HashMap` (not `std::HashMap`) so [`record_read`](Self::record_read)
    /// can take `&self` instead of `&mut self`. This is load-bearing for
    /// HIGH-C: the engine's tx-aware point read `TableManager::read_one_tx`
    /// holds the tx by shared reference (`Option<&TxContext>`) — the executor
    /// reborrows `&*tx` from a `&mut TxContext` — so an `&mut`-taking
    /// `record_read` could not be called from inside the read path without a
    /// signature break rippling into out-of-crate call sites. Interior
    /// mutability lets `read_one_tx` populate the read-set in place, which is
    /// what makes Serializable isolation actually detect write-skew (before
    /// this, `record_read` was wired only from unit tests, so the read-set
    /// was always empty in production and SSI silently degraded to Snapshot).
    pub read_set: scc::HashMap<(u64, Bytes), u64>,

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

    /// Unique-index guards recorded at stage time, re-validated under
    /// `commit_lock` (closes the tx-concurrent unique-violation hole).
    /// Each entry: the deterministic unique-index key this tx intends to
    /// own, plus the owning rid (so an update re-writing its own value is
    /// not a self-conflict). Discarded by RAII on abort like all other
    /// tx-local state. Not gated in [`is_empty`](Self::is_empty): a guard
    /// only ever accompanies a staged write, so it is never the sole
    /// occupant of the tx.
    pub unique_guards: Vec<UniqueGuard>,

    /// Predicate / range read-set for SSI phantom detection (Phase C).
    /// Populated ONLY when `isolation == Serializable`, exactly like
    /// [`read_set`](Self::read_set). Interior-mutable so the engine's
    /// scan path can append through a shared `&TxContext`.
    pub predicate_set: crate::predicate_set::PredicateSet,
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
            staged_vectors: HashMap::new(),
            interner_overlay: scc::HashMap::new(),
            next_overlay_id: AtomicU64::new(crate::layered_interner::OVERLAY_ID_BASE),
            counter_deltas: HashMap::new(),
            read_set: scc::HashMap::new(),
            table_tokens: HashMap::new(),
            version_provider: None,
            started_at: std::time::Instant::now(),
            unique_guards: Vec::new(),
            predicate_set: crate::predicate_set::PredicateSet::new(),
        }
    }

    /// Record a unique-index guard for commit-time re-validation.
    pub fn record_unique_guard(&mut self, g: UniqueGuard) {
        self.unique_guards.push(g);
    }

    /// How long this transaction has been open.
    pub fn elapsed(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    /// Whether this transaction has exceeded the given max lifetime.
    pub fn is_expired(&self, max_lifetime: std::time::Duration) -> bool {
        self.started_at.elapsed() > max_lifetime
    }

    /// Approximate byte footprint of everything this tx has staged.
    ///
    /// Mirrors the per-field set that `wal_ops_from_tx`
    /// (`crates/shamir-engine/src/tx/commit.rs:236`) materialises into the
    /// WAL entry: per-table staging (`write_set`), accumulated index ops
    /// (`index_write_set`), and tx-buffered HNSW vectors (`staged_vectors`).
    /// Counters, interner overlay, read-set, table tokens and unique guards
    /// are bounded bookkeeping and intentionally excluded — the cap is there
    /// to protect the *payload* dimension.
    ///
    /// Note: this measures the in-memory staging footprint, not the eventual
    /// `WalEntryV2` serialized length. The cap will trip somewhat earlier
    /// than the actual on-disk WAL size — fine for a protective budget.
    ///
    /// `O(N)` over staged entries; called once per `TxExecute` from the
    /// server's interactive-tx handler. Saturating arithmetic so a degenerate
    /// caller can never wrap.
    pub fn staged_bytes(&self) -> usize {
        let mut total: usize = 0;
        for staging in self.write_set.values() {
            total = total.saturating_add(staging.staged_bytes());
        }
        for (_token, op) in &self.index_write_set {
            match op {
                crate::IndexWriteOp::SetPosting { key, value } => {
                    total = total.saturating_add(key.len()).saturating_add(value.len());
                }
                crate::IndexWriteOp::RemovePosting { key } => {
                    total = total.saturating_add(key.len());
                }
                crate::IndexWriteOp::BumpFtsStats { .. } => {} // counter-only, no payload
            }
        }
        for vecs in self.staged_vectors.values() {
            for (_rid, embedding) in vecs {
                // 16 bytes of RecordId + 4 bytes per f32 lane.
                total = total
                    .saturating_add(16)
                    .saturating_add(embedding.len().saturating_mul(4));
            }
        }
        total
    }

    /// True if the tx has no pending writes / index ops / staging at all.
    pub fn is_empty(&self) -> bool {
        self.write_set.is_empty()
            && self.index_write_set.is_empty()
            && self.staged_vectors.is_empty()
            && self.interner_overlay.is_empty()
            && self.counter_deltas.is_empty()
    }

    /// Record a counter change for a table (e.g. +N for insert_many).
    pub fn bump_counter(&mut self, table_id: u64, delta: i64) {
        *self.counter_deltas.entry(table_id).or_insert(0) += delta;
    }

    /// Record a read for SSI validation (only if Serializable).
    ///
    /// `&mut self` overload, kept for existing call sites that hold the tx
    /// mutably (tests, benches, manually-driven read tracking). Delegates to
    /// [`record_read_shared`](Self::record_read_shared); both write the same
    /// interior-mutable `read_set`.
    pub fn record_read(&mut self, table_id: u64, key: Bytes, version: u64) {
        self.record_read_shared(table_id, key, version);
    }

    /// Record a read for SSI validation (only if Serializable), taking
    /// `&self` via interior mutability (`read_set` is an `scc::HashMap`).
    ///
    /// This is the entry point the engine's tx-aware read path uses:
    /// `TableManager::read_one_tx` holds the tx by shared reference
    /// (`Option<&TxContext>`), so it cannot call the `&mut self` overload.
    /// Wiring this in is what makes Serializable isolation actually populate
    /// the read-set in production (HIGH-C) — previously `record_read` was
    /// reachable only from unit tests, so `read_set` was always empty at
    /// commit and SSI silently degraded to Snapshot isolation.
    ///
    /// No-op under Snapshot isolation.
    ///
    /// **First-read-wins**: the version recorded for a key is the one observed
    /// at the *first* read; a later re-read of the same key does NOT overwrite
    /// it. This is the load-bearing SSI semantic. Versions are monotonic, so
    /// the first read captures the lowest (earliest) version the tx ever saw —
    /// the conservative bound for conflict detection. Overwriting with a newer
    /// version (last-write-wins, the previous `HashMap::insert` behaviour, fine
    /// only while reads were recorded once from unit tests) would mask a real
    /// conflict: e.g. an update's internal old-value read runs AFTER a
    /// concurrent committer bumped the key, and last-write-wins would re-record
    /// the key at the post-commit version, defeating the abort.
    pub fn record_read_shared(&self, table_id: u64, key: Bytes, version: u64) {
        if self.isolation == IsolationLevel::Serializable {
            use scc::hash_map::Entry::{Occupied, Vacant};
            match self.read_set.entry((table_id, key)) {
                // First-read-wins: keep the earliest observed version.
                Occupied(_) => {}
                Vacant(ve) => {
                    ve.insert_entry(version);
                }
            }
        }
    }

    /// Record a predicate dependency for SSI phantom detection.
    ///
    /// No-op under Snapshot isolation — zero-overhead invariant: the
    /// isolation gate runs BEFORE any work. Takes `&self` via interior
    /// mutability so engine scan paths can append through `&TxContext`.
    pub fn record_predicate_shared(&self, dep: crate::predicate_set::PredicateDep) {
        if self.isolation == IsolationLevel::Serializable {
            self.predicate_set.push(dep);
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
        // `scc::HashMap::scan` is a synchronous visitor `FnMut(&K, &V)` that
        // cannot early-return; capture the first conflict and report it after
        // the scan. Iteration order is unspecified (it was already with
        // `std::HashMap`), so which key surfaces on a multi-key conflict is
        // not contractual — callers test single-key scenarios.
        let mut conflict: Option<(u64, Bytes)> = None;
        self.read_set.scan(|(table_id, key), version_seen| {
            if conflict.is_some() {
                return;
            }
            match version_provider(*table_id, key) {
                None => conflict = Some((*table_id, key.clone())),
                Some(current) if current > *version_seen => {
                    conflict = Some((*table_id, key.clone()));
                }
                Some(_) => {}
            }
        });
        match conflict {
            Some(c) => Err(c),
            None => Ok(()),
        }
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

    /// Stage an HNSW vector under this tx for the given table token.
    ///
    /// The pair is buffered tx-locally and applied to the live graph at
    /// commit (Phase 5d). A dropped/aborted tx discards it (RAII) — the
    /// live graph is never touched until commit.
    pub fn stage_vector(&mut self, table_token: u64, rid: RecordId, vec: Vec<f32>) {
        self.staged_vectors
            .entry(table_token)
            .or_default()
            .push((rid, vec));
    }

    /// Vectors staged under this tx for `table_token`, for in-tx search
    /// merge. `None` when the table has no staged vectors.
    pub fn staged_vectors_for(&self, table_token: u64) -> Option<&[(RecordId, Vec<f32>)]> {
        self.staged_vectors.get(&table_token).map(Vec::as_slice)
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

    /// cancel-safe: NO — iterates `write_set` and invokes
    /// `rewrite_set_bytes` on each per-table StagingStore. Cancellation
    /// mid-iteration leaves a subset of tables remapped and the rest
    /// holding overlay ids — the tx must be aborted on error / cancel.
    ///
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
    fn stage_vector_buffers_per_table() {
        let mut ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
        ctx.stage_vector(10, RecordId([0u8; 16]), vec![1.0, 0.0]);
        ctx.stage_vector(10, RecordId([1u8; 16]), vec![0.0, 1.0]);
        ctx.stage_vector(20, RecordId([2u8; 16]), vec![1.0, 1.0]);

        assert_eq!(ctx.staged_vectors_for(10).map(<[_]>::len), Some(2));
        assert_eq!(ctx.staged_vectors_for(20).map(<[_]>::len), Some(1));
        assert_eq!(ctx.staged_vectors_for(30), None);
        assert!(!ctx.is_empty(), "staged vectors make the tx non-empty");
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

    #[tokio::test]
    async fn staged_bytes_accumulates_across_fields() {
        use shamir_storage::storage_in_memory::InMemoryStore;
        use shamir_storage::types::Store;

        let mut tx = TxContext::new(
            crate::types::TxId::new(1),
            0,
            10,
            crate::types::IsolationLevel::Snapshot,
        );

        // Empty tx → 0.
        assert_eq!(tx.staged_bytes(), 0);

        // Add a write_set entry: Set("k1", "val") → 2 + 3 = 5 bytes.
        let base: std::sync::Arc<dyn Store> = std::sync::Arc::new(InMemoryStore::new());
        let staging = tx.ensure_table_staging(42, "users", base);
        staging
            .set(Bytes::from_static(b"k1"), Bytes::from_static(b"val"))
            .await;
        let after_write = tx.staged_bytes();
        assert!(after_write > 0, "write_set should contribute");
        assert_eq!(after_write, 5);

        // Add an IndexWriteOp::SetPosting → key(3) + value(4) = 7 more.
        tx.index_write_set.push((
            42,
            crate::IndexWriteOp::SetPosting {
                key: Bytes::from_static(b"idx"),
                value: Bytes::from_static(b"post"),
            },
        ));
        let after_index = tx.staged_bytes();
        assert!(after_index > after_write, "index ops should add bytes");
        assert_eq!(after_index, 5 + 7);

        // Add a staged vector: 2-lane f32 → 16 (rid) + 2*4 = 24 bytes.
        tx.stage_vector(1, RecordId([0u8; 16]), vec![1.0, 2.0]);
        let after_vec = tx.staged_bytes();
        assert!(after_vec > after_index, "staged vectors should add bytes");
        assert_eq!(after_vec, 5 + 7 + 24);
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

    #[test]
    fn record_predicate_shared_noop_on_snapshot() {
        use crate::predicate_set::PredicateDep;
        let ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
        ctx.record_predicate_shared(PredicateDep::TableScan { table_token: 7 });
        ctx.record_predicate_shared(PredicateDep::IndexRange {
            table_token: 7,
            index_id: 1,
            lo: std::ops::Bound::Unbounded,
            hi: std::ops::Bound::Unbounded,
        });
        assert!(
            ctx.predicate_set.is_empty(),
            "Snapshot isolation must not record predicate deps"
        );
    }

    #[test]
    fn record_predicate_shared_appends_on_serializable() {
        use crate::predicate_set::PredicateDep;
        let ctx = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Serializable);
        ctx.record_predicate_shared(PredicateDep::TableScan { table_token: 7 });
        ctx.record_predicate_shared(PredicateDep::IndexRange {
            table_token: 7,
            index_id: 42,
            lo: std::ops::Bound::Included(bytes::Bytes::from_static(b"\x00")),
            hi: std::ops::Bound::Excluded(bytes::Bytes::from_static(b"\xff")),
        });
        assert_eq!(ctx.predicate_set.len(), 2);
    }

    // ── proptest: SSI read-set validation properties ──────────────────

    use proptest::prelude::*;
    use std::collections::HashMap as StdHashMap;

    /// Reference oracle: independently compute the expected validate_read_set
    /// outcome given the recorded reads and the provider map. Mirrors the
    /// IFF rule: Ok iff every recorded key has Some(current) with
    /// current <= version_seen.
    fn oracle_conflict(
        recorded: &StdHashMap<(u64, Vec<u8>), u64>,
        provider: &StdHashMap<(u64, Vec<u8>), Option<u64>>,
    ) -> bool {
        for ((t, k), version_seen) in recorded {
            match provider.get(&(*t, k.clone())) {
                None => return true,
                Some(None) => return true,
                Some(Some(current)) if *current > *version_seen => return true,
                Some(Some(_)) => {}
            }
        }
        false
    }

    /// Build a TxContext (Serializable) and replay the generated reads. Returns
    /// the tx plus the de-duplicated reference map (first-read-wins -> MIN).
    fn build_tx_with_reads(
        reads: &[(u64, Vec<u8>, u64)],
    ) -> (TxContext, StdHashMap<(u64, Vec<u8>), u64>) {
        let mut tx = TxContext::new(
            crate::types::TxId::new(1),
            0,
            10,
            crate::types::IsolationLevel::Serializable,
        );
        let mut recorded: StdHashMap<(u64, Vec<u8>), u64> = StdHashMap::new();
        for (t, k, v) in reads {
            tx.record_read(*t, Bytes::copy_from_slice(k), *v);
            recorded
                .entry((*t, k.clone()))
                .and_modify(|cur| {
                    if *v < *cur {
                        *cur = *v;
                    }
                })
                .or_insert(*v);
        }
        (tx, recorded)
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            .. ProptestConfig::default()
        })]

        /// Property 1 (AGREEMENT / IFF):
        /// `validate_read_set` returns Ok iff for every recorded read the
        /// provider yields Some(current) with current <= version_seen.
        #[test]
        fn prop_validate_read_set_iff_oracle(
            reads in proptest::collection::vec(
                (
                    0u64..4,
                    proptest::collection::vec(any::<u8>(), 1..=4),
                    0u64..=20,
                ),
                0..=8,
            ),
            provider_overrides in proptest::collection::vec(
                (0u64..=25, any::<bool>()),
                0..=8,
            ),
        ) {
            let (tx, recorded) = build_tx_with_reads(&reads);

            let mut provider_map: StdHashMap<(u64, Vec<u8>), Option<u64>> =
                StdHashMap::new();
            let keys: Vec<(u64, Vec<u8>)> = recorded.keys().cloned().collect();
            for (i, k) in keys.iter().enumerate() {
                if provider_overrides.is_empty() {
                    provider_map.insert(k.clone(), Some(0));
                } else {
                    let (cur, is_none) =
                        provider_overrides[i % provider_overrides.len()];
                    provider_map
                        .insert(k.clone(), if is_none { None } else { Some(cur) });
                }
            }

            let expected_conflict = oracle_conflict(&recorded, &provider_map);

            let pm_ref = &provider_map;
            let result = tx.validate_read_set(|t, k| {
                let kv: Vec<u8> = k.as_ref().to_vec();
                match pm_ref.get(&(t, kv)) {
                    Some(opt) => *opt,
                    None => Some(0),
                }
            });

            prop_assert_eq!(result.is_err(), expected_conflict);

            if let Err((conflict_t, conflict_k)) = result {
                let kv: Vec<u8> = conflict_k.as_ref().to_vec();
                prop_assert!(recorded.contains_key(&(conflict_t, kv.clone())));
                let version_seen = recorded[&(conflict_t, kv.clone())];
                let provided = pm_ref.get(&(conflict_t, kv)).copied();
                let is_real_conflict = match provided {
                    Some(None) => true,
                    Some(Some(cur)) => cur > version_seen,
                    None => false,
                };
                prop_assert!(
                    is_real_conflict,
                    "validate_read_set returned a non-conflicting key"
                );
            }
        }

        /// Property 2 (MONOTONE BUMP):
        /// Start from an exactly-passing provider (current = version_seen for
        /// every recorded read). Validation MUST succeed. Then, for any
        /// recorded key, bumping its current_version by `bump >= 1` MUST flip
        /// the result to a conflict.
        #[test]
        fn prop_validate_read_set_bump_creates_conflict(
            reads in proptest::collection::vec(
                (
                    0u64..4,
                    proptest::collection::vec(any::<u8>(), 1..=4),
                    0u64..=20,
                ),
                1..=8,
            ),
            pick in 0usize..1024,
            bump in 1u64..=50,
        ) {
            let (tx, recorded) = build_tx_with_reads(&reads);
            prop_assume!(!recorded.is_empty());

            let baseline: StdHashMap<(u64, Vec<u8>), u64> = recorded.clone();

            let baseline_ref = &baseline;
            let baseline_result = tx.validate_read_set(|t, k| {
                let kv: Vec<u8> = k.as_ref().to_vec();
                Some(*baseline_ref.get(&(t, kv)).unwrap_or(&0))
            });
            prop_assert!(
                baseline_result.is_ok(),
                "baseline provider (current == version_seen) must NOT conflict"
            );

            let keys: Vec<(u64, Vec<u8>)> = recorded.keys().cloned().collect();
            let target = keys[pick % keys.len()].clone();

            let mut bumped = baseline.clone();
            let target_seen = bumped[&target];
            bumped.insert(target.clone(), target_seen.saturating_add(bump));

            let bumped_ref = &bumped;
            let bumped_result = tx.validate_read_set(|t, k| {
                let kv: Vec<u8> = k.as_ref().to_vec();
                Some(*bumped_ref.get(&(t, kv)).unwrap_or(&0))
            });
            prop_assert!(
                bumped_result.is_err(),
                "bumping any recorded key's current_version above version_seen \
                 must trigger an SSI conflict"
            );
        }

        /// Property 3 (NONE-PROVIDER):
        /// If the provider returns None for ANY recorded key while every other
        /// key passes, validation must conflict.
        #[test]
        fn prop_validate_read_set_none_provider_is_conflict(
            reads in proptest::collection::vec(
                (
                    0u64..4,
                    proptest::collection::vec(any::<u8>(), 1..=4),
                    0u64..=20,
                ),
                1..=8,
            ),
            pick in 0usize..1024,
        ) {
            let (tx, recorded) = build_tx_with_reads(&reads);
            prop_assume!(!recorded.is_empty());

            let keys: Vec<(u64, Vec<u8>)> = recorded.keys().cloned().collect();
            let nilled = keys[pick % keys.len()].clone();
            let recorded_ref = &recorded;
            let nilled_ref = &nilled;
            let result = tx.validate_read_set(|t, k| {
                let kv: Vec<u8> = k.as_ref().to_vec();
                if (t, kv.clone()) == (nilled_ref.0, nilled_ref.1.clone()) {
                    None
                } else {
                    Some(*recorded_ref.get(&(t, kv)).unwrap_or(&0))
                }
            });
            prop_assert!(
                result.is_err(),
                "a None provider response for a recorded key must conflict"
            );
        }
    }
}
