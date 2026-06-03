use std::collections::BTreeSet;
use std::sync::Arc;

use futures::StreamExt;

use super::buffer_config;
use super::interner_manager::InternerManager;
use super::record_counter::RecordCounter;
use super::table::Table;
use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;
use crate::index::index_manager::IndexManager;
use crate::index::sorted_index_manager::SortedIndexManager;
use crate::query::filter::eval::{compile_filter, FilterCallback};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::Filter;
use shamir_storage::error::DbResult;
use shamir_storage::storage_membuffer::MemBufferConfig;
use shamir_storage::types::{KvOp, Store};
use shamir_types::core::interner::TouchInd;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::WalManager;

/// Bundled mutation effect for one record. Built by *_tx methods,
/// applied by `stage_mutation` to a TxContext.
struct StagedMutation {
    data_op: KvOp,
    index_ops: Vec<shamir_tx::IndexWriteOp>,
    counter_delta: i64,
}

/// Compute the deterministic token for a table name.
/// Same hash as `TableManager::table_token` (instance method).
pub fn table_token_for(name: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
}

pub struct TableManager {
    name: String,
    table: Arc<Table>,
    /// Direct handle to the info_store the sub-managers were
    /// built on. Kept so DDL (buffer-config get/set, future
    /// per-table settings) can hit the same store without going
    /// through any sub-manager's surface.
    info_store: Arc<dyn Store>,
    interner: InternerManager,
    counter: Arc<RecordCounter>,
    index_manager: IndexManager,
    sorted_indexes: SortedIndexManager,
    wal: Arc<WalManager>,
    /// Monotonic counter of mutating operations since open. The
    /// auto-verify background watchdog samples this; every
    /// `AUTO_VERIFY_EVERY_N_WRITES` operations it spawns a verify
    /// pass and logs anything unhealthy. See `bump_write_counter`.
    write_counter: Arc<std::sync::atomic::AtomicU64>,
    /// `true` when a background verify is in flight — prevents
    /// multiple concurrent verifies piling up.
    verify_running: Arc<std::sync::atomic::AtomicBool>,
    /// Serialises validate + write + index-update for tables that have
    /// unique indexes. Tables without unique indexes hit the fast path
    /// (no lock). `tokio::sync::Mutex` because the guard lives across
    /// `.await` points.
    unique_write_lock: Arc<tokio::sync::Mutex<()>>,
    index2_registry: Arc<crate::index2::IndexRegistry>,
    mvcc_store: Option<Arc<shamir_tx::MvccStore>>,
}

/// How often the background watchdog runs a `verify` pass.
/// Coarse — once per ~thousand mutating ops, regardless of batch
/// size. Tuned to "noticeable problem within seconds" without
/// noticeable overhead.
const AUTO_VERIFY_EVERY_N_WRITES: u64 = 1024;

impl Clone for TableManager {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            table: Arc::clone(&self.table),
            info_store: Arc::clone(&self.info_store),
            interner: self.interner.clone(),
            counter: Arc::clone(&self.counter),
            index_manager: self.index_manager.clone(),
            sorted_indexes: self.sorted_indexes.clone(),
            wal: Arc::clone(&self.wal),
            write_counter: Arc::clone(&self.write_counter),
            verify_running: Arc::clone(&self.verify_running),
            unique_write_lock: Arc::clone(&self.unique_write_lock),
            index2_registry: Arc::clone(&self.index2_registry),
            mvcc_store: self.mvcc_store.clone(),
        }
    }
}

impl TableManager {
    /// Create a new TableManager with all internal components.
    ///
    /// This is the preferred way to create a TableManager - it handles
    /// internal Table creation and all component initialization.
    pub async fn create(
        name: String,
        data_store: Arc<dyn Store>,
        info_store: Arc<dyn Store>,
    ) -> DbResult<Self> {
        let interner = InternerManager::new(Arc::clone(&info_store));
        let counter = Arc::new(RecordCounter::new(Arc::clone(&info_store)));
        let index_manager =
            IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store)).await?;
        let sorted_indexes = SortedIndexManager::new(Arc::clone(&info_store)).await?;
        let wal = Arc::new(WalManager::new(Arc::clone(&info_store)));
        let table = Table::new(data_store);

        let mgr = Self {
            name,
            table: Arc::new(table),
            info_store: Arc::clone(&info_store),
            interner,
            counter,
            index_manager,
            sorted_indexes,
            wal,
            write_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            verify_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            unique_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            index2_registry: Arc::new(crate::index2::IndexRegistry::new()),
            mvcc_store: None,
        };

        // Hot-load persisted buffer config (if any) and apply
        // it to both stores. If no DDL has set one, the stores
        // keep whatever default the factory wrapped them with.
        if let Some(cfg) = buffer_config::load(&mgr.info_store).await? {
            mgr.table.data_store().apply_buffer_config(&cfg).await?;
            mgr.info_store.apply_buffer_config(&cfg).await?;
        }

        // Restore index2 backends from persisted metadata.
        if let Some(persisted) =
            crate::index2::persistence::load_index2_metadata(&mgr.info_store).await?
        {
            mgr.index2_registry.set_next_id(persisted.next_id);
            for desc in persisted.descriptors {
                if matches!(desc.kind, crate::index2::kind::IndexKind::Btree { .. }) {
                    continue;
                }
                let backend = crate::index2::build_index2_backend(desc, &info_store);
                let _ = mgr.index2_registry.insert(backend).await;
            }
        }

        // Rebuild in-memory state from persisted data.
        // Vector backends lose their HNSW graph; FTS ranked backends
        // lose BM25 doc_count/sum_doc_len counters; others are no-op.
        {
            let backends = mgr.index2_registry.all_backends().await;
            for b in &backends {
                let data = Arc::clone(mgr.table.data_store());
                if let Err(e) = b.rebuild(data).await {
                    log::warn!(
                        "index2 rebuild failed for index {}: {}",
                        b.descriptor().name,
                        e
                    );
                }
            }
        }

        // Auto-recovery on open. Cheap on clean shutdown (one
        // prefix scan returning zero entries); targeted recovery
        // (O(batch_size)) or full repair (O(table_size)) when a
        // crash left WAL markers behind. Surfaces silently in the
        // log; the caller can also call `recover_on_open()`
        // explicitly later to receive the report.
        if let Some(report) = mgr.recover_on_open().await? {
            log::warn!(
                "Table '{}' opened with WAL markers — recovered {} record(s) in {} ms",
                mgr.name(),
                report.records_scanned,
                report.elapsed_ms,
            );
        }

        Ok(mgr)
    }

    /// Create a TableManager from existing components.
    ///
    /// This is primarily for testing or advanced use cases.
    #[cfg(test)]
    pub fn new(
        name: String,
        table: Table,
        interner: InternerManager,
        counter: Arc<RecordCounter>,
        index_manager: IndexManager,
    ) -> Self {
        // Tests that construct TableManager directly don't exercise
        // sorted indexes — give them an empty manager that shares
        // info_store... but we don't have it here. The simplest
        // thing: construct an "orphan" sorted manager backed by an
        // in-memory store. Its persisted defs blob then lives in a
        // throwaway store, which is fine because these tests never
        // call sorted-index methods.
        let info_store: Arc<dyn Store> =
            Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new());
        // Construct synchronously: SortedIndexManager::new() is async
        // but the empty-state path doesn't await any real work.
        let sorted_indexes =
            futures::executor::block_on(SortedIndexManager::new(info_store.clone()))
                .expect("sorted index manager init for test");
        let wal = Arc::new(WalManager::new(Arc::clone(&info_store)));
        Self {
            name,
            table: Arc::new(table),
            info_store,
            interner,
            counter,
            index_manager,
            sorted_indexes,
            wal,
            write_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            verify_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            unique_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            index2_registry: Arc::new(crate::index2::IndexRegistry::new()),
            mvcc_store: None,
        }
    }

    /// Increment the watchdog counter for `n` writes. Every
    /// `AUTO_VERIFY_EVERY_N_WRITES`-th increment spawns a
    /// non-blocking background `verify()` and logs at WARN if it
    /// reports inconsistency. Best-effort signal — does NOT block
    /// the caller, does NOT auto-repair (user calls `repair()`
    /// when ready).
    pub fn bump_write_counter(&self, n: u64) {
        use std::sync::atomic::Ordering;
        if n == 0 {
            return;
        }
        let prev = self.write_counter.fetch_add(n, Ordering::Relaxed);
        let next = prev.saturating_add(n);
        let crossed = prev / AUTO_VERIFY_EVERY_N_WRITES != next / AUTO_VERIFY_EVERY_N_WRITES;
        if !crossed {
            return;
        }
        // Single-flight: skip if another verify is in flight.
        if self
            .verify_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let self_clone = self.clone();
        tokio::spawn(async move {
            let result = self_clone.verify().await;
            match result {
                Ok(report) => {
                    if !report.is_healthy() {
                        log::warn!(
                            "Background verify flagged inconsistency in '{}': {:?}",
                            self_clone.name(),
                            report,
                        );
                    }
                }
                Err(e) => log::warn!("Background verify on '{}' failed: {}", self_clone.name(), e,),
            }
            self_clone.verify_running.store(false, Ordering::Release);
        });
    }

    /// Whether a background verify is currently in flight. Test-
    /// support accessor; users normally don't care.
    pub fn is_background_verify_running(&self) -> bool {
        use std::sync::atomic::Ordering;
        self.verify_running.load(Ordering::Acquire)
    }

    /// Public read-only access to the inner `Table` — used by
    /// `read_exec` for vectored `get_many` and by tests. Production
    /// callers must not write to the table directly; go through
    /// `TableManager::insert / set / delete` so index hooks fire.
    pub fn table(&self) -> &Table {
        &self.table
    }

    /// Stable u64 identifier for this table, used as key in
    /// `TxContext.write_set` and `counter_deltas`.
    ///
    /// Stage 4 implementation: deterministic hash of `self.name`.
    /// Stage 5 will replace with real repo-level interner ID.
    pub fn table_token(&self) -> u64 {
        table_token_for(&self.name)
    }

    /// Direct access to the underlying data_store. Used by V2 WAL
    /// recovery to apply Put/Delete ops bypassing the indexing /
    /// counter hooks (those replay separately).
    pub fn data_store(&self) -> &Arc<dyn Store> {
        self.table.data_store()
    }

    /// Borrow the info_store this table writes its sidecar
    /// metadata into (counter, interner dictionary, sorted-index
    /// blob, WAL, buffer config, ...). DDL uses this directly.
    pub fn info_store(&self) -> &Arc<dyn Store> {
        &self.info_store
    }

    // ---- Per-table buffer config (DDL surface) ----

    /// Read the persisted buffer config, if any. Returns `None`
    /// when no DDL has set one — the store still uses whatever
    /// default the factory wrapped it with.
    pub async fn get_buffer_config(&self) -> DbResult<Option<MemBufferConfig>> {
        buffer_config::load(&self.info_store).await
    }

    /// Persist a buffer config and hot-apply it to both stores.
    /// Idempotent — replays cleanly across restarts because the
    /// persisted value is reloaded on `TableManager::create`.
    ///
    /// cancel-safe: NO — persist → apply on data store → apply on
    /// info store. Cancellation between persist and apply leaves
    /// the live store config out of sync with the persisted value
    /// until the next restart (idempotent reload converges). Do NOT
    /// call under `tokio::select!` / `tokio::time::timeout`.
    pub async fn set_buffer_config(&self, cfg: &MemBufferConfig) -> DbResult<()> {
        buffer_config::save(&self.info_store, cfg).await?;
        self.table.data_store().apply_buffer_config(cfg).await?;
        self.info_store.apply_buffer_config(cfg).await?;
        Ok(())
    }

    /// Partial-update flavour: read the current persisted config
    /// (falling back to `MemBufferConfig::default()` if none was
    /// set), apply the closure to produce the new value, then
    /// persist and hot-apply. Convenient for "bump just one
    /// knob" DDL like `ALTER TABLE ... SET buffer.ttl_ms = ...`.
    pub async fn alter_buffer_config<F>(&self, mutate: F) -> DbResult<MemBufferConfig>
    where
        F: FnOnce(&mut MemBufferConfig),
    {
        let mut cfg = self.get_buffer_config().await?.unwrap_or_default();
        mutate(&mut cfg);
        self.set_buffer_config(&cfg).await?;
        Ok(cfg)
    }

    pub fn interner(&self) -> &InternerManager {
        &self.interner
    }

    /// Public accessor for the record counter — used by the read
    /// fast-path for `COUNT(*)` without filter (Opt #2).
    pub fn counter(&self) -> &Arc<RecordCounter> {
        &self.counter
    }

    #[cfg(test)]
    pub fn index_manager(&self) -> &IndexManager {
        &self.index_manager
    }

    /// Borrow the table's `IndexManager`. Public so the `db_instance`
    /// admin path (`create_index_async`) can register / drop indices via
    /// `TableManager` from outside this module — previously `pub(crate)`
    /// when this code was a single crate, but `db_instance` and
    /// `table_manager` now live in adjacent crate modules and the
    /// boundary needs `pub`.
    pub fn index_manager_ref(&self) -> &IndexManager {
        &self.index_manager
    }

    pub fn index2_registry(&self) -> &Arc<crate::index2::IndexRegistry> {
        &self.index2_registry
    }

    /// Clone the handle to this table's unique-write serialisation lock.
    ///
    /// HIGH-A — closing the non-tx ↔ tx-commit unique race. Non-tx
    /// `insert` / `set` / `delete` take this `tokio::sync::Mutex` around their
    /// validate-then-write-then-index window (see those methods). The tx
    /// commit pipeline (`commit_tx_inner` Phase 2.6 → 5c) acquires the SAME
    /// lock for every table that has unique guards, so a tx's unique
    /// re-check and its posting write are atomic against any concurrent non-tx
    /// unique writer to that table. Before this, the two paths used different
    /// mutexes (`unique_write_lock` vs the per-repo `commit_lock`), so a non-tx
    /// writer could claim/overwrite a unique key in the gap between the tx's
    /// Phase 2.6 check and its Phase 5c write — duplicate unique values and a
    /// corrupted posting. Returns the `Arc` (cloned) so the caller holds the
    /// exact same mutex instance the non-tx path locks.
    pub fn unique_write_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.unique_write_lock)
    }

    /// Borrow the table's sorted-index manager — used by the planner
    /// for range / order / min queries, and by DDL when a
    /// `create_index { sorted: true }` op lands.
    pub fn sorted_indexes(&self) -> &SortedIndexManager {
        &self.sorted_indexes
    }

    async fn index2_on_insert(&self, rid: &RecordId, rec: &InnerValue) {
        for backend in self.index2_registry.all_backends().await {
            if let Ok(ops) = backend.plan_insert(*rid, rec).await {
                let _ =
                    crate::index2::apply_index_ops(&ops, &self.info_store, backend.as_ref()).await;
            }
        }
    }

    async fn index2_on_update(&self, rid: &RecordId, old: &InnerValue, new: &InnerValue) {
        for backend in self.index2_registry.all_backends().await {
            if let Ok(ops) = backend.plan_update(*rid, old, new).await {
                let _ =
                    crate::index2::apply_index_ops(&ops, &self.info_store, backend.as_ref()).await;
            }
        }
    }

    async fn index2_on_delete(&self, rid: &RecordId, rec: &InnerValue) {
        for backend in self.index2_registry.all_backends().await {
            if let Ok(ops) = backend.plan_delete(*rid, rec).await {
                let _ =
                    crate::index2::apply_index_ops(&ops, &self.info_store, backend.as_ref()).await;
            }
        }
    }

    /// Apply a staged mutation to the TxContext.
    async fn stage_mutation(
        &self,
        m: StagedMutation,
        tx: &mut shamir_tx::TxContext,
    ) -> DbResult<()> {
        let staging = tx.ensure_table_staging(
            self.table_token(),
            &self.name,
            self.table.data_store().clone(),
        );
        match m.data_op {
            KvOp::Set(k, v) => staging.set(k, v).await,
            KvOp::Remove(k) => staging.remove(k).await,
        }
        let token = self.table_token();
        tx.index_write_set
            .extend(m.index_ops.into_iter().map(|op| (token, op)));
        tx.bump_counter(self.table_token(), m.counter_delta);
        Ok(())
    }

    /// Collect index ops from all index2 backends for an insert.
    /// Does NOT apply — ops go into tx.index_write_set for deferred apply.
    ///
    /// `tx_id` is forwarded to each backend's `plan_insert_tx` so
    /// backends that maintain non-storage state (e.g. `VectorBackend`'s
    /// HNSW graph) can route the mutation into a per-tx staging area
    /// instead of the live structure (HIGH-6). Stateless backends
    /// (FTS / functional / btree) fall through to `plan_insert` via
    /// the default trait impl.
    async fn plan_insert_ops(
        &self,
        rid: RecordId,
        rec: &InnerValue,
        tx_id: Option<shamir_tx::TxId>,
    ) -> Vec<shamir_tx::IndexWriteOp> {
        let mut all_ops = Vec::new();
        for backend in self.index2_registry.all_backends().await {
            if let Ok(ops) = backend.plan_insert_tx(rid, rec, tx_id).await {
                all_ops.extend(ops);
            }
        }
        all_ops
    }

    /// HIGH-6: route any HNSW vectors carried by `rec` into the tx's own
    /// `staged_vectors` buffer instead of the live graph. Each vector
    /// backend extracts its embedding (`IndexBackend::staged_vector`);
    /// the `(rid, vec)` pair lands under this table's token. Promoted
    /// into the graph atomically at commit (Phase 5d), discarded by RAII
    /// on abort. Stateless backends return `None` and contribute nothing.
    ///
    /// The tx-aware `plan_*_tx` methods deliberately leave the live graph
    /// untouched (no-op for `Some(tx)`), so this is the sole staging path.
    async fn stage_vectors(&self, rid: RecordId, rec: &InnerValue, tx: &mut shamir_tx::TxContext) {
        let token = self.table_token();
        for backend in self.index2_registry.all_backends().await {
            if let Some(v) = backend.staged_vector(rid, rec).await {
                tx.stage_vector(token, rid, v);
            }
        }
    }

    /// Collect index ops from all index2 backends for an update.
    /// Does NOT apply — ops go into tx.index_write_set for deferred apply.
    ///
    /// See [`plan_insert_ops`] for the `tx_id` parameter.
    async fn plan_update_ops(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
        tx_id: Option<shamir_tx::TxId>,
    ) -> Vec<shamir_tx::IndexWriteOp> {
        let mut all_ops = Vec::new();
        for backend in self.index2_registry.all_backends().await {
            if let Ok(ops) = backend.plan_update_tx(rid, old, new, tx_id).await {
                all_ops.extend(ops);
            }
        }
        all_ops
    }

    /// Collect index ops from all index2 backends for a delete.
    /// Does NOT apply — ops go into tx.index_write_set for deferred apply.
    ///
    /// See [`plan_insert_ops`] for the `tx_id` parameter.
    async fn plan_delete_ops(
        &self,
        rid: RecordId,
        rec: &InnerValue,
        tx_id: Option<shamir_tx::TxId>,
    ) -> Vec<shamir_tx::IndexWriteOp> {
        let mut all_ops = Vec::new();
        for backend in self.index2_registry.all_backends().await {
            if let Ok(ops) = backend.plan_delete_tx(rid, rec, tx_id).await {
                all_ops.extend(ops);
            }
        }
        all_ops
    }

    /// HIGH-6: collect legacy `IndexManager` (regular + unique) and
    /// `SortedIndexManager` posting ops for a tx insert. These ops use
    /// the *exact* physical key layout the non-tx readers expect
    /// (`lookup_by_index` / `check_unique_constraint` / `lookup_range`),
    /// so applying them at commit time produces postings indistinguishable
    /// from the non-tx `on_record_created` path. The unique ops do NOT
    /// validate — validation runs separately at stage time in `insert_tx`.
    async fn plan_legacy_insert_ops(
        &self,
        rid: RecordId,
        rec: &InnerValue,
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>> {
        let mut ops = self.index_manager.plan_record_created(&rid, rec).await?;
        ops.extend(
            self.index_manager
                .plan_record_created_unique(&rid, rec)
                .await?,
        );
        ops.extend(self.sorted_indexes.plan_record_created(&rid, rec)?);
        Ok(ops)
    }

    /// HIGH-6: legacy + sorted posting ops for a tx update.
    async fn plan_legacy_update_ops(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>> {
        let mut ops = self
            .index_manager
            .plan_record_updated(&rid, old, new)
            .await?;
        ops.extend(
            self.index_manager
                .plan_record_updated_unique(&rid, old, new)
                .await?,
        );
        ops.extend(self.sorted_indexes.plan_record_updated(&rid, old, new)?);
        Ok(ops)
    }

    /// HIGH-6: legacy + sorted posting ops for a tx delete.
    async fn plan_legacy_delete_ops(
        &self,
        rid: RecordId,
        old: &InnerValue,
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>> {
        let mut ops = self.index_manager.plan_record_deleted(&rid, old).await?;
        ops.extend(
            self.index_manager
                .plan_record_deleted_unique(&rid, old)
                .await?,
        );
        ops.extend(self.sorted_indexes.plan_record_deleted(&rid, old)?);
        Ok(ops)
    }

    /// tx-aware insert.
    ///
    /// - `tx == None` → delegates to existing [`insert`].
    /// - `tx == Some` → stages data + index ops + counter delta in
    ///   TxContext. No physical writes. commit_tx Phase 5 applies.
    ///
    /// HIGH-6: legacy `IndexManager` (regular + unique) and
    /// `SortedIndexManager` posting writes ARE now staged into
    /// `tx.index_write_set` via [`plan_legacy_insert_ops`]. The planners
    /// emit `IndexWriteOp`s carrying the exact physical key layout the
    /// non-tx readers expect, so the commit pipeline applies them
    /// atomically and a dropped tx leaves no ghost postings. Unique-index
    /// validation runs at stage time below (read-only `validate_unique_for_create`)
    /// to reject duplicates early.
    ///
    /// tx-concurrent unique violation: stage-time validation reads
    /// committed state only, so two concurrent txs inserting the same
    /// unique value both pass it. The hole is now closed by recording a
    /// `UniqueGuard` per claimed unique key here; `commit_tx` Phase 2.6
    /// re-validates each guard under `commit_lock` (the same
    /// serialisation point the non-tx path gets from `unique_write_lock`).
    /// The unique key is deterministic in the value, so the commit-time
    /// `info_store.get(index_key)` settles ownership byte-for-byte.
    ///
    /// HIGH-6: stateful HNSW vectors are routed tx-locally — the index2
    /// `plan_insert_tx` is a no-op on the live graph for a tx, and
    /// [`stage_vectors`] buffers the embedding in `tx.staged_vectors`.
    /// Stateless peers (FTS / functional / btree) emit `IndexWriteOp`s
    /// accumulated in `tx.index_write_set`. A successful commit applies
    /// both (`commit_tx` Phase 5c for postings, Phase 5d for vectors); a
    /// dropped tx discards both by RAII.
    pub async fn insert_tx(
        &self,
        value: &InnerValue,
        tx: Option<&mut shamir_tx::TxContext>,
    ) -> DbResult<RecordId> {
        let Some(tx) = tx else {
            return self.insert(value).await;
        };

        let rid = RecordId::new();

        // HIGH-6: stage-time unique validation (read-only against
        // committed state). Optimistic fast-reject for the common
        // single-writer duplicate; the tx-concurrent case is settled by
        // the commit-time guard below.
        self.index_manager.validate_unique_for_create(value).await?;

        // Record a UniqueGuard per unique key this value claims, so
        // commit_tx Phase 2.6 re-validates it under commit_lock (closes
        // the two-concurrent-txs hole). The recorded key is byte-identical
        // to what check_unique_constraint reads at commit time.
        for index_key in self.index_manager.unique_keys_for(value) {
            tx.record_unique_guard(shamir_tx::UniqueGuard {
                table_token: self.table_token(),
                index_key,
                owner: rid,
            });
        }

        let bytes = value.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("Failed to serialize InnerValue: {}", e))
        })?;

        let tx_id = Some(tx.tx_id);
        let mut index_ops = self.plan_insert_ops(rid, value, tx_id).await;
        index_ops.extend(self.plan_legacy_insert_ops(rid, value).await?);

        // HIGH-6: stage HNSW vectors tx-locally (not into the live graph).
        self.stage_vectors(rid, value, tx).await;

        self.stage_mutation(
            StagedMutation {
                data_op: KvOp::Set(rid.to_bytes(), bytes),
                index_ops,
                counter_delta: 1,
            },
            tx,
        )
        .await?;

        Ok(rid)
    }

    /// Batched tx-aware insert — mirrors [`insert_many`] for the tx
    /// staging path. Stages N records' data + index ops + counter
    /// delta into `tx` in one pass, lifting the per-row overhead
    /// (`validate_unique_for_create`, `unique_keys_for`,
    /// `all_backends().await` snapshots, `plan_legacy_insert_ops`)
    /// out of the row loop.
    ///
    /// Semantics MUST match calling [`insert_tx`] N times:
    ///   * each row gets a fresh `RecordId` (returned in input order);
    ///   * `UniqueGuard`s are recorded one-per-claim-per-row (the
    ///     `owner` is the row's rid — commit_tx Phase 2.6 needs that
    ///     to settle ownership);
    ///   * HNSW vectors are staged tx-locally via [`stage_vector`];
    ///   * stateless index2 backends emit ops via `plan_insert_tx`;
    ///   * legacy / unique / sorted indexes emit ops via the existing
    ///     batch planners (`plan_records_created_batch`,
    ///     `plan_records_created_unique_batch`, sorted-by-def loop);
    ///   * counter delta = +N is bumped once;
    ///   * all per-row data writes go through one `ensure_table_staging`
    ///     handle (still one `staging.set` per row — StagingStore has
    ///     no public set_many, and the underlying overlay is a
    ///     per-tx in-memory map → no fsync amplification).
    ///
    /// Returns the assigned ids in input order. Empty input returns
    /// an empty Vec without touching `tx`.
    pub async fn insert_tx_many(
        &self,
        values: &[InnerValue],
        tx: &mut shamir_tx::TxContext,
    ) -> DbResult<Vec<RecordId>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }

        // 1. Batch-validate unique indexes. Mirrors `insert_many`:
        //    persisted check + batch-local seen set (so two rows in
        //    ONE batch claiming the same unique value reject the
        //    later one rather than silently overwriting).
        if self.index_manager.has_unique_indexes() {
            let mut batch_seen: std::collections::HashSet<(u64, Vec<u8>)> =
                std::collections::HashSet::new();
            for (i, v) in values.iter().enumerate() {
                self.index_manager.validate_unique_for_create(v).await?;
                for def in self.index_manager.iter_unique_indexes() {
                    if let Some(vs) =
                        crate::index::index_manager::IndexManager::extract_index_values(
                            v, &def.paths,
                        )
                    {
                        let key = bincode::serialize(&vs)
                            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                        if !batch_seen.insert((def.name_interned, key)) {
                            return Err(shamir_storage::error::DbError::DuplicateKey(format!(
                                "Unique index '{}' violated within batch (row {} duplicates an earlier row)",
                                def.name_interned, i
                            )));
                        }
                    }
                }
            }
        }

        // 2. Generate ids + serialise bytes upfront.
        let mut ids: Vec<RecordId> = Vec::with_capacity(values.len());
        let mut rows_bytes: Vec<bytes::Bytes> = Vec::with_capacity(values.len());
        for v in values {
            ids.push(RecordId::new());
            let b = v.to_bytes().map_err(|e| {
                shamir_storage::error::DbError::Codec(format!(
                    "Failed to serialize InnerValue: {}",
                    e
                ))
            })?;
            rows_bytes.push(b);
        }

        // 3. Record UniqueGuards per row per unique index it claims.
        //    Same shape as `insert_tx` (one guard per claimed key,
        //    `owner = rid`) — commit_tx Phase 2.6 settles ownership
        //    per guard, byte-identical to the per-row staging path.
        if self.index_manager.has_unique_indexes() {
            let token = self.table_token();
            for (rid, v) in ids.iter().zip(values.iter()) {
                for index_key in self.index_manager.unique_keys_for(v) {
                    tx.record_unique_guard(shamir_tx::UniqueGuard {
                        table_token: token,
                        index_key,
                        owner: *rid,
                    });
                }
            }
        }

        // 4. Take the index2 backend snapshot ONCE, then drive both
        //    plan_insert_tx (stateless ops → index_write_set) and
        //    staged_vector (HNSW → tx.staged_vectors) per row off the
        //    cached list. This is the main per-row→batched lift:
        //    `all_backends().await` walks the scc::HashMap; doing it
        //    once amortises across N rows.
        let backends = self.index2_registry.all_backends().await;
        let tx_id = Some(tx.tx_id);
        let token = self.table_token();
        let mut index_ops: Vec<shamir_tx::IndexWriteOp> = Vec::new();
        for (rid, v) in ids.iter().zip(values.iter()) {
            for backend in &backends {
                if let Ok(ops) = backend.plan_insert_tx(*rid, v, tx_id).await {
                    index_ops.extend(ops);
                }
                if let Some(vec) = backend.staged_vector(*rid, v).await {
                    tx.stage_vector(token, *rid, vec);
                }
            }
        }

        // 5. Legacy + sorted batch planners — one call each, planning
        //    over the whole (id, value) iterator. Same physical key
        //    layout the non-tx readers expect (see
        //    `plan_legacy_insert_ops` for the contract).
        let pairs = || ids.iter().zip(values.iter());
        let mut legacy_ops = self
            .index_manager
            .plan_records_created_batch(pairs())
            .await?;
        legacy_ops.extend(
            self.index_manager
                .plan_records_created_unique_batch(pairs())
                .await?,
        );
        legacy_ops.extend(self.sorted_indexes.plan_records_created_batch(pairs())?);
        index_ops.extend(legacy_ops);

        // 6. Single ensure_table_staging, then a tight set loop. We
        //    can't fold the data writes into index_write_set, but the
        //    StagingStore overlay is a per-tx in-memory map — no
        //    fsync per set, all hot in cache. One handle replaces
        //    N `tx.write_set.entry(...).or_insert_with(...)` walks.
        let staging = tx.ensure_table_staging(token, &self.name, self.table.data_store().clone());
        for (rid, b) in ids.iter().zip(rows_bytes.into_iter()) {
            staging.set(rid.to_bytes(), b).await;
        }

        // 7. Merge index_ops + counter delta in one go.
        tx.index_write_set
            .extend(index_ops.into_iter().map(|op| (token, op)));
        tx.bump_counter(token, values.len() as i64);

        Ok(ids)
    }

    /// tx-aware update.
    ///
    /// `tx == None` → existing `set` (since `update` is currently
    /// internal helper; `set` is the public surface).
    /// `tx == Some` → reads old value via `read_one_tx` (write_set or
    /// main store), plans diff index ops via `plan_update`, stages
    /// the new bytes.
    ///
    /// Returns `true` if a record was already present (semantically
    /// matches existing `set`).
    ///
    /// HIGH-6: see `insert_tx` for the staging contract and the
    /// commit-time application gap.
    pub async fn update_tx(
        &self,
        id: RecordId,
        value: &InnerValue,
        tx: Option<&mut shamir_tx::TxContext>,
    ) -> DbResult<bool> {
        let Some(tx) = tx else {
            return self.set(id, value).await;
        };

        let old = self.read_one_tx(id, Some(&*tx)).await.ok();

        // HIGH-6: stage-time unique validation (read-only). For an
        // existing record this excludes the record itself; for a fresh
        // insert it behaves like create-validation.
        match &old {
            Some(old_val) => {
                self.index_manager
                    .validate_unique_for_update(&id, old_val, value)
                    .await?
            }
            None => self.index_manager.validate_unique_for_create(value).await?,
        }

        // Record a UniqueGuard per unique key the NEW value claims, owner
        // = the rid being updated. commit_tx Phase 2.6 re-validates under
        // commit_lock; an update re-writing its own value sees
        // `existing == owner` and is not a self-conflict.
        for index_key in self.index_manager.unique_keys_for(value) {
            tx.record_unique_guard(shamir_tx::UniqueGuard {
                table_token: self.table_token(),
                index_key,
                owner: id,
            });
        }

        let bytes = value.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("Failed to serialize InnerValue: {}", e))
        })?;

        let tx_id = Some(tx.tx_id);
        let (mut index_ops, counter_delta) = match &old {
            Some(old_val) => (self.plan_update_ops(id, old_val, value, tx_id).await, 0_i64),
            None => (self.plan_insert_ops(id, value, tx_id).await, 1_i64),
        };
        match &old {
            Some(old_val) => {
                index_ops.extend(self.plan_legacy_update_ops(id, old_val, value).await?)
            }
            None => index_ops.extend(self.plan_legacy_insert_ops(id, value).await?),
        }

        // HIGH-6: stage the new vector tx-locally (apply_committed_vectors
        // upsert-replaces the prior committed entry at commit time).
        self.stage_vectors(id, value, tx).await;

        self.stage_mutation(
            StagedMutation {
                data_op: KvOp::Set(id.to_bytes(), bytes),
                index_ops,
                counter_delta,
            },
            tx,
        )
        .await?;

        Ok(old.is_some())
    }

    /// tx-aware delete.
    ///
    /// `tx == None` → existing `delete`.
    /// `tx == Some` → reads old value, plans delete ops, stages
    /// Remove. Returns `true` if a record was present.
    ///
    /// HIGH-6: see `insert_tx` for the staging contract and the
    /// commit-time application gap.
    pub async fn delete_tx(
        &self,
        id: RecordId,
        tx: Option<&mut shamir_tx::TxContext>,
    ) -> DbResult<bool> {
        let Some(tx) = tx else {
            return self.delete(id).await;
        };

        let Some(old) = self.read_one_tx(id, Some(&*tx)).await.ok() else {
            return Ok(false);
        };

        let tx_id = Some(tx.tx_id);
        let mut index_ops = self.plan_delete_ops(id, &old, tx_id).await;
        index_ops.extend(self.plan_legacy_delete_ops(id, &old).await?);

        self.stage_mutation(
            StagedMutation {
                data_op: KvOp::Remove(id.to_bytes()),
                index_ops,
                counter_delta: -1,
            },
            tx,
        )
        .await?;

        Ok(true)
    }

    /// tx-aware insert-or-update by RecordId. Alias of [`update_tx`]
    /// — same semantics in tx mode.
    pub async fn set_tx(
        &self,
        id: RecordId,
        value: &InnerValue,
        tx: Option<&mut shamir_tx::TxContext>,
    ) -> DbResult<bool> {
        self.update_tx(id, value, tx).await
    }

    /// Register a new sorted (B-tree-by-value) index over a single
    /// scalar field, then backfill it from existing records.
    ///
    /// cancel-safe: NO — `register` persists the definition, then
    /// the backfill streams existing rows into the new index.
    /// Cancellation after register but before/during the backfill
    /// loop leaves a registered sorted index with partial entries;
    /// the doctor's `repair()` rebuilds the index from scratch as a
    /// recovery path. Do NOT call under `tokio::select!` /
    /// `tokio::time::timeout`.
    pub async fn create_sorted_index(&self, index_name: &str, field_path: &[&str]) -> DbResult<()> {
        use crate::index::sorted_index_manager::SortedIndexDefinition;
        let interner = self.interner.get().await?;
        let name_interned = interner
            .touch_ind(index_name)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?
            .key()
            .id();
        let mut path_ids: Vec<u64> = Vec::new();
        for seg in field_path {
            for part in seg.split('.') {
                let id = interner
                    .touch_ind(part)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?
                    .key()
                    .id();
                path_ids.push(id);
            }
        }
        let def = SortedIndexDefinition::new(name_interned, path_ids);
        self.sorted_indexes.register(def).await?;
        self.interner.persist().await?;

        // Backfill: stream existing records and add each to the new
        // sorted index. Avoids materialising the whole table.
        use futures::StreamExt;
        let stream = self.table.list_stream(1000);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (id, record) in batch? {
                self.sorted_indexes.on_record_created(&id, &record).await?;
            }
        }
        Ok(())
    }

    /// Drop a sorted index by name.
    pub async fn drop_sorted_index(&self, index_name: &str) -> DbResult<bool> {
        let interner = self.interner.get().await?;
        let Some(name_interned) = interner.get_ind(index_name) else {
            return Ok(false);
        };
        self.sorted_indexes.drop_index(name_interned.id()).await
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Insert an InnerValue, returns RecordId (with counter and index update)
    ///
    /// Validates unique indexes BEFORE insert, returns error if constraint violated.
    ///
    /// cancel-safe: NO — sequence is data-write → counter-bump → 3 index
    /// updates with no WAL marker around it (unlike `insert_many` which
    /// uses `wal.begin_with_delta`/`commit`). Cancellation between the
    /// data write (`self.table.insert`) and the index hooks leaves the
    /// data store with orphan records that the indexes don't see; the
    /// doctor's `repair()` pass is the recovery path. Do NOT call this
    /// under `tokio::select!` or `tokio::time::timeout` — use
    /// `insert_many(&[value])` for the WAL-covered single-record path.
    pub async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        let _guard = if self.index_manager.has_unique_indexes() {
            Some(self.unique_write_lock.lock().await)
        } else {
            None
        };

        // 1. Validate unique indexes BEFORE write
        self.index_manager.validate_unique_for_create(value).await?;

        // 2. Write to table. Route through MvccStore (SSI / version cache
        //    + history archival under active snapshots) when one is
        //    attached; otherwise fall back to a direct data_store write.
        //    Pre-generating the RecordId here lets us use the keyed
        //    `set_versioned` path instead of `Table::insert`'s auto-key
        //    `data_store.insert`. The MvccStore writes to `main` (same
        //    physical layout as direct `set`), so observers reading via
        //    `data_store.get` see the new record identically.
        let id = RecordId::new();
        let bytes = value.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("Failed to serialize InnerValue: {}", e))
        })?;
        if let Some(mvcc) = &self.mvcc_store {
            mvcc.set_versioned(id.to_bytes(), bytes).await?;
        } else {
            self.table.data_store().set(id.to_bytes(), bytes).await?;
        }
        self.counter.increment(1).await?;

        // 3. Update indexes AFTER write
        self.index_manager.on_record_created(&id, value).await?;
        self.index_manager
            .on_record_created_unique(&id, value)
            .await?;
        self.sorted_indexes.on_record_created(&id, value).await?;
        self.index2_on_insert(&id, value).await;

        Ok(id)
    }

    /// Batched insert of N records. Validates unique indexes first
    /// for all values, then issues one batched `Table::insert_many`
    /// (which dispatches to `Store::insert_many` — on nebari / persy /
    /// redb that's a single transaction = one fsync for the data
    /// store). Counter increments by N once; index updates still
    /// loop per-record (a follow-up sprint can batch the index
    /// writes through `info_store.set_many`).
    ///
    /// Atomicity matches `Store::insert_many` for the chosen backend
    /// (transactional all-or-nothing on nebari / persy / redb;
    /// per-record on backends using the default loop impl).
    pub async fn insert_many(&self, values: &[InnerValue]) -> DbResult<Vec<RecordId>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }

        // 1. Validate unique indexes for every value first. Two
        //    layers of check: persisted state (via
        //    `validate_unique_for_create`) AND batch-local seen
        //    map, because two rows within ONE batch with the same
        //    unique value would otherwise both pass the persisted
        //    check and silently overwrite each other in step 3.
        if self.index_manager.has_unique_indexes() {
            // Map: (unique_index_name_interned, encoded_values_key)
            // → first index in the batch that claimed it. Cheap
            // bincode-based key avoids fighting `InnerValue` hash
            // requirements (Map keyed by interner ids isn't `Hash`).
            let mut batch_seen: std::collections::HashSet<(u64, Vec<u8>)> =
                std::collections::HashSet::new();
            for (i, v) in values.iter().enumerate() {
                self.index_manager.validate_unique_for_create(v).await?;
                // Now record this row's unique-index claims so the
                // next iteration sees them.
                for def in self.index_manager.iter_unique_indexes() {
                    if let Some(vs) =
                        crate::index::index_manager::IndexManager::extract_index_values(
                            v, &def.paths,
                        )
                    {
                        let key = bincode::serialize(&vs)
                            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                        if !batch_seen.insert((def.name_interned, key)) {
                            return Err(shamir_storage::error::DbError::DuplicateKey(format!(
                                "Unique index '{}' violated within batch (row {} duplicates an earlier row)",
                                def.name_interned, i
                            )));
                        }
                    }
                }
            }
        }

        // 2. Data-store write. When an MvccStore is attached, route the
        //    whole batch through `set_versioned_many` (III.4) so
        //    `version_cache` and history archival stay consistent with
        //    non-tx writes WHILE collapsing the main writes into a single
        //    `Store::transact` — one fsync instead of N on backends that
        //    override `transact`. The previous per-record `set_versioned`
        //    loop re-introduced the N× fsync amplification this path now
        //    avoids. Without an MvccStore we keep the legacy batched path
        //    (one transaction = one fsync on backends that override
        //    `insert_many`).
        let ids: Vec<RecordId> = if let Some(mvcc) = &self.mvcc_store {
            let mut ids = Vec::with_capacity(values.len());
            let mut items = Vec::with_capacity(values.len());
            for v in values {
                let rid = RecordId::new();
                let bytes = v.to_bytes().map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "Failed to serialize InnerValue: {}",
                        e
                    ))
                })?;
                items.push((rid.to_bytes(), bytes));
                ids.push(rid);
            }
            mvcc.set_versioned_many(items).await?;
            ids
        } else {
            self.table.insert_many(values).await?
        };

        // 3. Open a WAL marker — records the record_ids we just
        //    inserted so that crash recovery can scope its check
        //    to exactly these records. The marker write is one
        //    info_store.set call; on backends with eventual flush
        //    (sled / redb-Durability::None) it amortises through
        //    the same flush window as the rest of the batch — no
        //    extra fsync on the happy path.
        //
        //    Gap: a crash between step 2 and step 3 leaves orphan
        //    records in data_store with no WAL marker; doctor's
        //    full-rebuild (`TableManager::repair()`) handles that
        //    fallback case.
        let txn_id = self.wal.fresh_txn_id();
        self.wal
            .begin_with_delta(
                txn_id,
                shamir_wal::WalManager::ops_record_created(&ids),
                ids.len() as i64,
            )
            .await?;

        // 4. counter + indexes (all in info_store).
        self.counter.increment(ids.len() as i64).await?;
        let pairs_iter = || ids.iter().zip(values.iter());
        self.index_manager
            .on_records_created_batch(pairs_iter())
            .await?;
        self.index_manager
            .on_records_created_unique_batch(pairs_iter())
            .await?;
        self.sorted_indexes
            .on_records_created_batch(pairs_iter())
            .await?;
        for (id, value) in pairs_iter() {
            self.index2_on_insert(id, value).await;
        }

        // 5. Clear the WAL marker — durable batch from here on.
        self.wal.commit(txn_id).await?;

        // 6. Bump the watchdog. Every AUTO_VERIFY_EVERY_N_WRITES
        //    operations a background verify fires and logs any
        //    inconsistency. Non-blocking, single-flight, best-
        //    effort signal.
        self.bump_write_counter(ids.len() as u64);

        Ok(ids)
    }

    /// Read-only access to the WAL — for recovery callsites and
    /// integration tests.
    pub fn wal(&self) -> &Arc<WalManager> {
        &self.wal
    }

    /// Delete a record by RecordId (with counter and index update)
    ///
    /// cancel-safe: NO — data-delete → counter decrement → 3 index
    /// deletes without WAL coverage. Cancellation after the data delete
    /// but before the index hooks leaves orphan index entries (a record
    /// the data store no longer has but the indexes still point to).
    /// The batch path `execute_delete` uses WAL; this single-record path
    /// does not. Do NOT call this under `tokio::select!` or
    /// `tokio::time::timeout`.
    pub async fn delete(&self, id: RecordId) -> DbResult<bool> {
        // Get old value before deletion for index cleanup
        let old_value = self.table.get(id).await.ok();
        // Route through MvccStore when attached so the old bytes are
        // archived to history under active snapshots and `version_cache`
        // is bumped. `delete_versioned` returns `()`; we treat the
        // pre-read as the source of truth for "removed".
        let removed = if let Some(mvcc) = &self.mvcc_store {
            if old_value.is_some() {
                mvcc.delete_versioned(id.to_bytes()).await?;
                true
            } else {
                false
            }
        } else {
            self.table.delete(id).await?
        };
        if removed {
            self.counter.increment(-1).await?;
            if let Some(ref old) = old_value {
                self.index_manager.on_record_deleted(&id, old).await?;
                self.index_manager
                    .on_record_deleted_unique(&id, old)
                    .await?;
                self.sorted_indexes.on_record_deleted(&id, old).await?;
                self.index2_on_delete(&id, old).await;
            }
        }
        Ok(removed)
    }

    /// Set a record by RecordId - creates if not exists, updates if exists (with counter and index update)
    ///
    /// Validates unique indexes BEFORE write, returns error if constraint violated.
    ///
    /// cancel-safe: NO — read-then-validate-then-write-then-index-update
    /// without WAL coverage. Cancellation between the table write and
    /// the index hooks leaves stale index entries (indexes point at the
    /// previous value while the data store holds the new one). Use the
    /// batch path (`execute_update` / `insert_many`) when atomicity
    /// matters; do NOT call this under `tokio::select!` or
    /// `tokio::time::timeout`.
    pub async fn set(&self, id: RecordId, value: &InnerValue) -> DbResult<bool> {
        let _guard = if self.index_manager.has_unique_indexes() {
            Some(self.unique_write_lock.lock().await)
        } else {
            None
        };

        // Get old value before update for index maintenance
        let old_value = self.table.get(id).await.ok();

        // 1. Validate unique indexes BEFORE write
        if let Some(ref old) = old_value {
            self.index_manager
                .validate_unique_for_update(&id, old, value)
                .await?;
        } else {
            self.index_manager.validate_unique_for_create(value).await?;
        }

        // 2. Write to table. Route through MvccStore when attached so
        //    `version_cache` is updated for SSI conflict detection and
        //    the old bytes are archived to history under active snapshots.
        //    `created` is derived from the pre-read above (same semantics
        //    as the previous `self.table.set` which internally did the
        //    same exists-check).
        let bytes = value.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("Failed to serialize InnerValue: {}", e))
        })?;
        let created = old_value.is_none();
        if let Some(mvcc) = &self.mvcc_store {
            mvcc.set_versioned(id.to_bytes(), bytes).await?;
        } else {
            self.table.data_store().set(id.to_bytes(), bytes).await?;
        }

        // 3. Update indexes AFTER write
        if created {
            self.counter.increment(1).await?;
            self.index_manager.on_record_created(&id, value).await?;
            self.index_manager
                .on_record_created_unique(&id, value)
                .await?;
            self.sorted_indexes.on_record_created(&id, value).await?;
            self.index2_on_insert(&id, value).await;
        } else if let Some(old) = old_value {
            self.index_manager
                .on_record_updated(&id, &old, value)
                .await?;
            self.index_manager
                .on_record_updated_unique(&id, &old, value)
                .await?;
            self.sorted_indexes
                .on_record_updated(&id, &old, value)
                .await?;
            self.index2_on_update(&id, &old, value).await;
        }
        Ok(created)
    }

    /// Count records (uses stored counter for O(1) performance)
    pub async fn count(&self) -> DbResult<usize> {
        Ok(self.counter.get().await? as usize)
    }

    /// Stream records in batches, returning InnerValues
    ///
    /// This is memory-efficient for large tables as it doesn't load all records at once.
    /// Returns a stream that yields batches of records.
    ///
    /// # Arguments
    /// * `batch_size` - Number of records per batch
    ///
    /// # Returns
    /// A stream that yields batches of (RecordId, InnerValue) tuples
    pub fn list_stream(
        &self,
        batch_size: usize,
    ) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> {
        self.table.list_stream(batch_size)
    }

    /// Stream records filtered by a compiled filter callback.
    ///
    /// Compiles the Filter AST into a callback network, then yields
    /// batches of matching records. The filter is compiled once; only
    /// matching records are yielded — non-matching records are dropped
    /// immediately without accumulation.
    ///
    /// # Arguments
    /// * `batch_size` - Number of records per batch from storage
    /// * `filter` - Filter AST to compile and apply
    /// * `ctx` - Filter context with interner and resolved query refs
    pub async fn filter_stream<'a>(
        &self,
        batch_size: usize,
        filter: &Filter,
        ctx: &'a FilterContext<'a>,
    ) -> DbResult<impl futures::Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> + 'a> {
        let interner = self.interner.get().await?;
        let callback = compile_filter(filter, interner);
        let table_stream = self.table.list_stream(batch_size);

        Ok(async_stream::stream! {
            futures::pin_mut!(table_stream);
            while let Some(batch_result) = table_stream.next().await {
                match batch_result {
                    Err(e) => { yield Err(e); return; }
                    Ok(batch) => {
                        let filtered: Vec<_> = batch
                            .into_iter()
                            .filter(|(_, record)| callback.matches(record, ctx))
                            .collect();
                        if !filtered.is_empty() {
                            yield Ok(filtered);
                        }
                    }
                }
            }
        })
    }

    /// Stream records filtered by a pre-compiled callback.
    ///
    /// Use this when you want to compile the filter once and reuse it.
    pub fn filter_stream_with_callback<'a>(
        &self,
        batch_size: usize,
        callback: &'a dyn FilterCallback,
        ctx: &'a FilterContext<'a>,
    ) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> + 'a {
        let table_stream = self.table.list_stream(batch_size);

        async_stream::stream! {
            futures::pin_mut!(table_stream);
            while let Some(batch_result) = table_stream.next().await {
                match batch_result {
                    Err(e) => { yield Err(e); return; }
                    Ok(batch) => {
                        let filtered: Vec<_> = batch
                            .into_iter()
                            .filter(|(_, record)| callback.matches(record, ctx))
                            .collect();
                        if !filtered.is_empty() {
                            yield Ok(filtered);
                        }
                    }
                }
            }
        }
    }

    /// tx-aware streaming variant of [`list_stream`].
    ///
    /// Forwards to [`list_stream`] for the actual data, then — when `tx` is
    /// `Some` and the tx is Serializable — records each *materialised*
    /// record's key into the read-set (HIGH-C). The yielded batches are
    /// byte-for-byte the same as [`list_stream`]; recording is a pure
    /// side-effect threaded lazily through the stream, so the lazy-yield
    /// contract is preserved (a consumer that stops early only records the
    /// keys it actually pulled).
    ///
    /// Streaming-scan SSI scope: this records the keys the scan *yields*. It
    /// does NOT install predicate / range locks, so phantom inserts into the
    /// scanned range by a concurrent tx are not detected — full SSI predicate
    /// locking over a stream is a known harder problem and out of scope here.
    /// Point reads ([`read_one_tx`]) and materialised scan reads are covered.
    ///
    /// KNOWN LIMITATION — no read-your-own-writes for scans. Streaming scans
    /// do NOT overlay the tx's own `write_set`: a record this tx staged
    /// (inserted/updated/deleted but not yet committed) is **invisible** to an
    /// in-tx stream — a staged insert is absent, a staged delete still appears,
    /// a staged update yields the committed value. Only point reads
    /// ([`read_one_tx`]) overlay staging (read-your-own-writes). A staged
    /// change becomes visible to a scan only after commit. Full streaming RYOW
    /// (inject staged inserts, drop staged deletes, override staged updates,
    /// all while preserving the lazy-yield + SSI recording contract) is a
    /// follow-up. The `list_stream_tx_does_not_see_staged_insert` test pins
    /// this current behaviour so the limitation cannot regress silently.
    pub fn list_stream_tx<'a>(
        &'a self,
        tx: Option<&'a shamir_tx::TxContext>,
        batch_size: usize,
    ) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> + 'a {
        // Phase C (Step 6): defensive coarse recording for streams
        // reached directly (bypassing read_tx). Zero-overhead: gate on
        // Serializable before any work.
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::TableScan {
                    table_token: self.table_token(),
                });
            }
        }
        let inner = self.list_stream(batch_size);
        let token = self.table_token();
        let mvcc = self.mvcc_store.clone();
        Self::record_scan_reads(inner, tx, token, mvcc)
    }

    /// tx-aware streaming variant of [`filter_stream`].
    ///
    /// Same materialised-read SSI recording as [`list_stream_tx`]: each
    /// record that survives the filter and is yielded gets recorded into the
    /// read-set (Serializable only). Same streaming-scan SSI scope note, and
    /// the same KNOWN LIMITATION: scans do NOT overlay the tx `write_set`
    /// (no read-your-own-writes for scans — see [`list_stream_tx`]).
    pub async fn filter_stream_tx<'a>(
        &'a self,
        tx: Option<&'a shamir_tx::TxContext>,
        batch_size: usize,
        filter: &Filter,
        ctx: &'a FilterContext<'a>,
    ) -> DbResult<impl futures::Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> + 'a> {
        // Phase C (Step 6): predicate recording for filter streams
        // reached directly (bypassing read_tx). Zero-overhead: gate on
        // Serializable before any work.
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let token = self.table_token();
                let deps = crate::query::filter::eval::predicate_to_index_range(
                    filter,
                    self.sorted_indexes(),
                    ctx.interner,
                    token,
                );
                if deps.is_empty() {
                    t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::TableScan {
                        table_token: token,
                    });
                } else {
                    for d in deps {
                        t.record_predicate_shared(d);
                    }
                }
            }
        }
        let inner = self.filter_stream(batch_size, filter, ctx).await?;
        let token = self.table_token();
        let mvcc = self.mvcc_store.clone();
        Ok(Self::record_scan_reads(inner, tx, token, mvcc))
    }

    /// tx-aware streaming variant of [`filter_stream_with_callback`].
    ///
    /// Same materialised-read SSI recording as [`list_stream_tx`], and the
    /// same KNOWN LIMITATION: scans do NOT overlay the tx `write_set` (no
    /// read-your-own-writes for scans — see [`list_stream_tx`]).
    pub fn filter_stream_with_callback_tx<'a>(
        &'a self,
        tx: Option<&'a shamir_tx::TxContext>,
        batch_size: usize,
        callback: &'a dyn FilterCallback,
        ctx: &'a FilterContext<'a>,
    ) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> + 'a {
        let inner = self.filter_stream_with_callback(batch_size, callback, ctx);
        let token = self.table_token();
        let mvcc = self.mvcc_store.clone();
        Self::record_scan_reads(inner, tx, token, mvcc)
    }

    /// Wrap a record stream so that, for a Serializable tx, each yielded
    /// record's key is recorded into the read-set at the version observed
    /// when it is pulled. The wrapper is transparent: it yields exactly what
    /// the inner stream yields, in the same order. For `tx == None` or a
    /// non-Serializable tx it adds no per-record work beyond a single
    /// up-front isolation check (the `version_of` lookup and recording are
    /// skipped entirely). `mvcc` is `None` → version `0` (conservative
    /// default, see [`read_one_tx`]).
    fn record_scan_reads<'a, S>(
        inner: S,
        tx: Option<&'a shamir_tx::TxContext>,
        token: u64,
        mvcc: Option<Arc<shamir_tx::MvccStore>>,
    ) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> + 'a
    where
        S: futures::Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> + 'a,
    {
        // Only Serializable txs track reads; everything else is a pass-through
        // so the non-SSI scan path pays nothing per record.
        let recording_tx = tx.filter(|t| t.isolation == shamir_tx::IsolationLevel::Serializable);
        async_stream::stream! {
            futures::pin_mut!(inner);
            while let Some(batch_result) = inner.next().await {
                if let (Ok(batch), Some(tx)) = (&batch_result, recording_tx) {
                    for (rid, _) in batch {
                        let key = rid.to_bytes();
                        let version = mvcc.as_ref().map_or(0, |m| m.version_of(key.as_ref()));
                        tx.record_read_shared(token, key, version);
                    }
                }
                yield batch_result;
            }
        }
    }

    /// Get a record by RecordId
    pub async fn get(&self, id: RecordId) -> DbResult<InnerValue> {
        self.table.get(id).await
    }

    /// Attach an MvccStore for tx-aware reads. Returns `self` so callers
    /// can chain after `create()`. When attached, `read_one_tx(rid, Some(tx))`
    /// reads through the MvccStore at `tx.snapshot_version`. Without an
    /// attached MvccStore, tx-aware reads fall through to the non-tx
    /// fast path (same as `get`).
    pub fn with_mvcc_store(mut self, mvcc: Arc<shamir_tx::MvccStore>) -> Self {
        self.mvcc_store = Some(mvcc);
        self
    }

    /// tx-aware single-record read.
    ///
    /// - `tx == None` → same as [`get`]: direct read from main data_store.
    /// - `tx == Some(tx)` and no `mvcc_store` attached → same as [`get`].
    /// - `tx == Some(tx)` and `mvcc_store` attached →
    ///   `mvcc.get_at(rid.to_bytes(), tx.snapshot_version)`:
    ///     - `Some(bytes)` → deserialize and return.
    ///     - `None` → `DbError::NotFound`.
    ///
    /// I.4 — read-your-own-writes. Before consulting the snapshot base, the
    /// tx's own staging overlay (`tx.write_set[token]`, the `StagingStore`
    /// holding this tx's un-committed set/remove ops) is checked for `key`:
    ///   - staged `Set(bytes)` → return the staged value (read-your-own-write);
    ///   - staged `Remove`     → return `NotFound` (read-your-own-delete);
    ///   - not staged          → fall through to `get_at(snapshot)` (the base).
    ///
    /// This makes a write performed earlier in the same tx visible to a later
    /// read in that tx — the fundamental tx-semantics guarantee — and is
    /// isolation-independent (applies to both Snapshot and Serializable). It
    /// also makes repeated in-tx updates of the same record index-correct:
    /// `update_tx`/`delete_tx` read the *staged* prior value as "old" when
    /// computing index diffs, so a second update of a key staged earlier in
    /// the same tx diffs against what the tx actually staged, not the base.
    ///
    /// The staging overlay is classified via
    /// [`StagingStore::staged_op`](shamir_tx::staging_store::StagingStore::staged_op):
    /// a targeted, alloc-free per-key probe (a single `scc::HashMap::read`
    /// borrow-probe with `&[u8]`, no `Bytes` allocation and no fall-through to
    /// the base store). A key has at most one staged op (last-write-wins), and
    /// the staged `Bytes` clone on a `Set` hit is an O(1) refcount bump. This
    /// is O(1) per read and allocates nothing when the key is not staged — it
    /// replaces the former `snapshot_ops()` path, which allocated a fresh
    /// `Vec<KvOp>` and cloned every staged op for the table on *every* read,
    /// then linearly scanned it for one key (O(N) per read for a tx that
    /// staged N rows). Semantics are unchanged: the probe only runs when this
    /// table has staged writes (guarded by the `write_set` lookup), so a tx
    /// that never wrote this table still pays nothing.
    ///
    /// Scope: this is the *point-read* overlay. The streaming scans
    /// ([`list_stream_tx`] / [`filter_stream_tx`] /
    /// [`filter_stream_with_callback_tx`]) do NOT yet merge the tx's own
    /// staged inserts/deletes/updates into the yielded records — streaming
    /// read-your-own-writes (inject staged inserts, drop staged deletes,
    /// override staged updates, all while preserving the lazy-yield + SSI
    /// recording contract) is a larger change tracked as a follow-up.
    ///
    /// HIGH-C — SSI read tracking. When `tx` is `Some`, the key and the
    /// version observed at read time are recorded into the tx's read-set via
    /// [`record_read_shared`](shamir_tx::TxContext::record_read_shared) (a
    /// no-op under Snapshot isolation, so callers pay nothing there). This is
    /// what makes Serializable isolation actually detect write-skew: commit
    /// Phase 2 re-checks every recorded key's current committed version, and
    /// aborts if any advanced past the version seen here. Before this wiring,
    /// `record_read` was reachable only from unit tests, so the read-set was
    /// always empty in production and Serializable silently degraded to
    /// Snapshot.
    ///
    /// The version is sourced from the table's `MvccStore::version_of` (the
    /// same handle the `RepoVersionProvider` queries at commit, so the
    /// captured and re-checked versions are directly comparable). When no
    /// `MvccStore` is attached we record version `0` — the conservative
    /// default: `0 <= any current version`, so an unwritten / untracked key
    /// never spuriously conflicts, while a later real write still surfaces.
    ///
    /// `record_read_shared` takes `&self` (interior-mutable read-set) — this
    /// is why the read-set can be populated through the existing
    /// `Option<&TxContext>` signature without forcing a `&mut` borrow that
    /// would ripple into every call site.
    pub async fn read_one_tx(
        &self,
        id: RecordId,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<InnerValue> {
        if let Some(tx) = tx {
            let key = id.to_bytes();
            // Capture the version at read time, then record the SSI read
            // dependency. No-op for Snapshot isolation.
            let version = self
                .mvcc_store
                .as_ref()
                .map_or(0, |mvcc| mvcc.version_of(key.as_ref()));
            tx.record_read_shared(self.table_token(), key.clone(), version);

            // I.4 read-your-own-writes: the tx's own staging overlay wins
            // over the snapshot base. A targeted per-key probe (alloc-free,
            // no fall-through to base): staged Set → return the staged value,
            // staged Remove → NotFound, not staged → fall through to the
            // snapshot base below. Only the table's own staging is probed
            // (guarded by the write_set lookup), so a tx that never wrote this
            // table pays nothing.
            if let Some(staging) = tx.write_set.get(&self.table_token()) {
                match staging.staged_op(key.as_ref()) {
                    Some(shamir_tx::staging_store::StagedKind::Set(v)) => {
                        return InnerValue::from_bytes(v).map_err(|e| {
                            shamir_storage::error::DbError::Codec(format!(
                                "Failed to deserialize InnerValue: {}",
                                e
                            ))
                        });
                    }
                    Some(shamir_tx::staging_store::StagedKind::Removed) => {
                        return Err(shamir_storage::error::DbError::NotFound(format!(
                            "record staged-removed in tx: {:?}",
                            id
                        )));
                    }
                    None => {}
                }
            }

            if let Some(mvcc) = self.mvcc_store.as_ref() {
                match mvcc.get_at(key.as_ref(), tx.snapshot_version).await? {
                    Some(bytes) => {
                        return InnerValue::from_bytes(bytes).map_err(|e| {
                            shamir_storage::error::DbError::Codec(format!(
                                "Failed to deserialize InnerValue: {}",
                                e
                            ))
                        });
                    }
                    None => {
                        return Err(shamir_storage::error::DbError::NotFound(format!(
                            "record not found at snapshot {}: {:?}",
                            tx.snapshot_version, id
                        )));
                    }
                }
            }
        }
        self.table.get(id).await
    }

    /// tx-aware read-query execution (Vector I.1).
    ///
    /// The wire `execute_batch` path dispatches `BatchOp::Read` here when a
    /// `transactional` batch is in flight, so a Serializable batch's SELECT
    /// populates the tx read-set and SSI write-skew detection becomes live
    /// end-to-end. Without this the executor called [`read`] directly with no
    /// tx, leaving `read_set` empty and Serializable silently degraded to
    /// Snapshot (the last unwired hop after HIGH-C wired the point/stream
    /// `*_tx` read APIs).
    ///
    /// The returned [`QueryResult`] is **byte-for-byte** [`read`]'s output —
    /// this method calls [`read`] for the authoritative result and treats the
    /// read-set recording as a pure side-effect. It does NOT change snapshot
    /// visibility of the scan: like the existing `execute_plan_tx` reads, the
    /// projection still observes live committed state (snapshot-visible scans
    /// are a separate, harder change confined to the read pipeline).
    ///
    /// Cost model:
    /// - `tx == None` → exactly [`read`], no extra work (non-tx reads are not
    ///   regressed).
    /// - `tx == Some(Snapshot)` → exactly [`read`]; `record_read_shared` is a
    ///   no-op off Serializable, so we skip the recording scan entirely.
    /// - `tx == Some(Serializable)` → [`read`] plus one recording pass over the
    ///   records matching the query's WHERE (via the existing tx-aware streams,
    ///   which record each yielded key at its observed version). This is the
    ///   only path that pays for SSI tracking.
    ///
    /// SSI scope: the recording pass walks every record matching the WHERE
    /// filter (or the whole table when there is no WHERE), so it records the
    /// keys the query logically reads. It records by point key only — it does
    /// NOT install predicate / range locks, so a concurrent tx inserting a NEW
    /// row into the scanned predicate (a phantom) is not detected. For a query
    /// with pagination/limit it conservatively records the full matching set
    /// rather than just the returned page (over-recording is SSI-safe: it can
    /// only ever add a conflict, never miss one).
    pub async fn read_tx(
        &self,
        query: &crate::query::read::ReadQuery,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<crate::query::read::QueryResult> {
        // Only a Serializable tx records reads; for everything else the
        // recording pass is pure overhead, so dispatch straight to `read`.
        let recording = tx
            .filter(|t| t.isolation == shamir_tx::IsolationLevel::Serializable)
            .is_some();
        if recording {
            // Phase C (Step 6): one-shot predicate-set recording derived
            // from query.r#where. Zero-overhead: this block only runs for
            // Serializable txs.
            let tx_ref = tx.expect("recording=true implies tx is Some");
            let token = self.table_token();
            match query.r#where.as_ref() {
                None => {
                    // No WHERE → coarse TableScan over the whole table.
                    tx_ref.record_predicate_shared(
                        shamir_tx::predicate_set::PredicateDep::TableScan { table_token: token },
                    );
                }
                Some(filter) => {
                    let deps = crate::query::filter::eval::predicate_to_index_range(
                        filter,
                        self.sorted_indexes(),
                        ctx.interner,
                        token,
                    );
                    if deps.is_empty() {
                        tx_ref.record_predicate_shared(
                            shamir_tx::predicate_set::PredicateDep::TableScan {
                                table_token: token,
                            },
                        );
                    } else {
                        for dep in deps {
                            tx_ref.record_predicate_shared(dep);
                        }
                    }
                }
            }

            // Existing per-record recording pass (unchanged) — captures
            // point-key reads for the read_set, complementing the
            // predicate_set.
            self.record_query_reads(query, ctx, tx).await?;
        }
        self.read(query, ctx).await
    }

    /// Record SSI read dependencies for `query` into the tx read-set.
    ///
    /// Reuses the existing tx-aware scan streams ([`filter_stream_tx`] /
    /// [`list_stream_tx`]) purely for their per-record recording side-effect:
    /// draining the stream records each matching record's key at the version
    /// observed when it is pulled. The yielded batches are discarded — only the
    /// read-set mutation matters here. Caller guarantees `tx` is `Some` and
    /// Serializable (the streams self-gate, but we never reach here otherwise).
    async fn record_query_reads(
        &self,
        query: &crate::query::read::ReadQuery,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<()> {
        let batch_size = 1000;
        match query.r#where.as_ref() {
            Some(filter) => {
                let stream = self.filter_stream_tx(tx, batch_size, filter, ctx).await?;
                futures::pin_mut!(stream);
                while let Some(batch) = stream.next().await {
                    batch?;
                }
            }
            None => {
                let stream = self.list_stream_tx(tx, batch_size);
                futures::pin_mut!(stream);
                while let Some(batch) = stream.next().await {
                    batch?;
                }
            }
        }
        Ok(())
    }

    // ============================================================================
    // Migration support — index2 cutover
    // ============================================================================

    /// Replicate src's interner state into this TableManager's info_store.
    ///
    /// Migration copies raw `data_store` bytes, which embed `InternerKey(u64)`
    /// references for field names. For those bytes to decode correctly on
    /// dst, dst's interner must hold the **same** `id → name` mappings as
    /// src. The interner persists itself under a fixed system key, so we
    /// just copy that one record byte-for-byte and let the dst interner
    /// pick it up via its normal lazy-load path.
    ///
    /// Must be called BEFORE any `.interner().get()` on `self` (so the
    /// lazy load sees the freshly-copied bytes) and BEFORE
    /// `replicate_index2_descriptors_from` (which re-interns names on dst).
    /// Persists src first so the bytes are current.
    pub async fn replicate_interner_from(&self, src: &TableManager) -> DbResult<()> {
        src.interner().persist().await?;
        // Copy the legacy single-blob record (if any) — for repos
        // upgraded from the old persist format. New code never writes
        // here, but on-disk data may still contain it.
        let legacy_key = crate::meta::MetaKey::Internals.as_record_id().to_bytes();
        match src.info_store.get(legacy_key.clone()).await {
            Ok(bytes) => {
                self.info_store.set(legacy_key, bytes).await?;
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => {
                // No legacy blob — fall through to chunk copy.
            }
            Err(e) => return Err(e),
        }
        // Copy every append-only delta chunk emitted by the new
        // incremental persist path. Without this, the dst would load
        // an empty interner and msgpack-encoded field-name ids on dst
        // would not resolve.
        use futures::StreamExt;
        let prefix = {
            let mut p = Vec::with_capacity(4 + 3);
            p.extend_from_slice(&[0u8, 0, 0, 0]);
            p.extend_from_slice(b"i.d");
            bytes::Bytes::from(p)
        };
        let mut stream = src.info_store.scan_prefix_stream(prefix, 256);
        while let Some(batch) = stream.next().await {
            for (k, v) in batch? {
                self.info_store.set(k, v).await?;
            }
        }
        Ok(())
    }

    /// Replicate src's index2 descriptors onto this TableManager.
    ///
    /// For each non-Btree descriptor on `src`:
    /// 1. Intern the name + path segments in **this** manager's interner
    ///    (so `name_interned` and `paths` resolve correctly in the dst
    ///    address space).
    /// 2. Allocate a fresh local `id` (src ids are a separate counter).
    /// 3. Build a backend via `build_index2_backend` (empty — no postings
    ///    yet) and register it in this registry.
    /// 4. Persist metadata.
    ///
    /// Must be called **before** `bulk_populate_index2` and **before**
    /// any writes reach the dst table.
    pub async fn replicate_index2_descriptors_from(&self, src: &TableManager) -> DbResult<()> {
        let src_backends = src.index2_registry.all_backends().await;
        if src_backends.is_empty() {
            return Ok(());
        }

        let interner = self.interner.get().await?;

        for src_backend in &src_backends {
            let src_desc = src_backend.descriptor();

            // Intern name in dst address space.
            let name_key = match interner
                .touch_ind(&src_desc.name)
                .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?
            {
                TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
            };

            // Intern each path segment in dst address space.
            let mut interned_paths: smallvec::SmallVec<[Vec<u64>; 2]> = smallvec::SmallVec::new();
            for path in &src_desc.paths {
                let mut seg_ids = Vec::with_capacity(path.len());
                // The src's `paths` already contain interned u64s from
                // src's interner — but we need the original field names
                // to re-intern on dst. Recover them via src's interner.
                let src_interner = src.interner.get().await?;
                for &seg_u64 in path {
                    let seg_str = src_interner
                        .get_str(&src_interner.make_key(seg_u64))
                        .ok_or_else(|| {
                            shamir_storage::error::DbError::Internal(format!(
                                "cannot resolve interned segment {} from src",
                                seg_u64
                            ))
                        })?;
                    let dst_key = match interner
                        .touch_ind(seg_str)
                        .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?
                    {
                        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
                    };
                    seg_ids.push(dst_key);
                }
                interned_paths.push(seg_ids);
            }

            let new_id = self.index2_registry.allocate_id();
            let new_desc = crate::index2::descriptor::IndexDescriptor::new(
                new_id,
                src_desc.name.clone(),
                name_key,
                interned_paths,
                src_desc.kind.clone(),
            );

            let backend = crate::index2::build_index2_backend(new_desc, &self.info_store);
            self.index2_registry
                .insert(backend)
                .await
                .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
        }

        self.interner.persist().await?;
        crate::index2::persistence::save_index2_metadata(&self.index2_registry, &self.info_store)
            .await?;

        Ok(())
    }

    /// Bulk-populate all index2 backends by streaming records from this
    /// TableManager's data_store and calling `plan_insert + apply_index_ops`
    /// for each record on every registered backend.
    ///
    /// This creates postings in info_store **and** populates in-memory
    /// state (HNSW graph, BM25 counters, etc.). Intended for migration
    /// cutover — the dst table has data_store populated by snapshot/drain
    /// but its info_store is empty.
    ///
    /// Must be called **after** `replicate_index2_descriptors_from` and
    /// **after** all data has landed in the dst data_store (i.e. after
    /// `drain_until_caught_up`). New writes after `bulk_populate_index2`
    /// must go through `insert()` which calls `index2_on_insert` — the
    /// migration coordinator's `final_drain_and_commit` writes directly
    /// to `dst_data` (data_store only) and does **not** trigger index2
    /// hooks. Therefore `bulk_populate_index2` should be called **after**
    /// `final_drain_and_commit` if any shadow-log entries may have been
    /// written between `drain_until_caught_up` and `mark_cutover_ready`.
    pub async fn bulk_populate_index2(&self) -> DbResult<()> {
        let backends = self.index2_registry.all_backends().await;
        if backends.is_empty() {
            return Ok(());
        }

        let stream = self.table.list_stream(1000);
        futures::pin_mut!(stream);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            let items: Vec<(RecordId, &InnerValue)> =
                batch.iter().map(|(rid, val)| (*rid, val)).collect();
            for backend in &backends {
                for (rid, val) in items.iter() {
                    let ops = backend.plan_insert(*rid, val).await.map_err(|e| {
                        shamir_storage::error::DbError::Internal(format!(
                            "bulk_populate_index2 plan_insert failed: {}",
                            e
                        ))
                    })?;
                    crate::index2::apply_index_ops(&ops, &self.info_store, backend.as_ref())
                        .await
                        .map_err(|e| {
                            shamir_storage::error::DbError::Internal(format!(
                                "bulk_populate_index2 apply_index_ops failed: {}",
                                e
                            ))
                        })?;
                }
            }
        }

        Ok(())
    }

    // ============================================================================
    // Index Management API (string paths → interned internally)
    // ============================================================================

    /// Create a regular index on specified paths.
    ///
    /// # Arguments
    /// * `name` - Index name (will be interned)
    /// * `paths` - Field paths, e.g. `["email"]` or `["user", "address.city"]`
    ///
    /// # Example
    /// ```ignore
    /// table.create_index("email_idx", &["email"]).await?;
    /// table.create_index("name_city_idx", &["name", "address.city"]).await?;
    /// ```
    pub async fn create_index_v2(
        &self,
        op: &shamir_query_types::admin::CreateIndexOp,
    ) -> DbResult<()> {
        use crate::index2::backend::IndexBackend;
        use crate::index2::descriptor::IndexDescriptor;
        use crate::index2::kind::*;
        use smallvec::SmallVec;

        let index_type = op.index_type.as_deref().unwrap_or("btree");
        if index_type == "btree" {
            let paths: Vec<String> = op.fields.iter().map(|segs| segs.join(".")).collect();
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
            return if op.unique {
                self.create_unique_index(&op.create_index, &path_refs).await
            } else {
                self.create_index(&op.create_index, &path_refs).await
            };
        }

        let interner = self.interner.get().await?;
        let mut interned_paths: SmallVec<[Vec<u64>; 2]> = SmallVec::new();
        for field_path in &op.fields {
            let mut seg_ids = Vec::with_capacity(field_path.len());
            for seg in field_path {
                let key = match interner
                    .touch_ind(seg)
                    .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?
                {
                    TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
                };
                seg_ids.push(key);
            }
            interned_paths.push(seg_ids);
        }

        let id = self.index2_registry.allocate_id();
        let name_key = match interner
            .touch_ind(&op.create_index)
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?
        {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };

        let first_path = interned_paths.first().cloned().unwrap_or_default();

        let (_kind, backend): (IndexKind, Arc<dyn IndexBackend>) = match index_type {
            "fts" => {
                // DSL names for fts_tokenizer:
                //   "whitespace"          → plain whitespace split
                //   "unicode"             → unicode-aware split
                //   "stemmed_<lang>"      → Full { <lang>, stopwords=true, stem=true }
                //       where <lang> is a full name or 2-letter ISO code:
                //       en/english, ru/russian, fr/french, de/german,
                //       es/spanish, it/italian, pt/portuguese, nl/dutch,
                //       sv/swedish, no/norwegian, da/danish, fi/finnish,
                //       hu/hungarian, ro/romanian, tr/turkish, el/greek,
                //       ar/arabic, ta/tamil
                //   "ngram"               → Ngram { n: 3 } (default trigram)
                //   "ngram2".."ngram9"    → Ngram { n: <digit> }
                let tok = fts_tokenizer_from_dsl(op.fts_tokenizer.as_deref());
                let kind = IndexKind::Fts {
                    tokenizer: tok,
                    language: op.fts_language.clone(),
                };
                let desc = IndexDescriptor::new(
                    id,
                    &op.create_index,
                    name_key,
                    interned_paths.clone(),
                    kind.clone(),
                );
                let backend: Arc<dyn IndexBackend> =
                    Arc::new(crate::index2::fts_ranked_backend::FtsRankedBackend::new(
                        desc,
                        first_path,
                        Arc::clone(self.info_store()),
                    ));
                (kind, backend)
            }
            "functional" => {
                let expr_op = op.functional_op.as_deref().unwrap_or("lower");
                let base = crate::index2::expr::IndexExpr::Field(first_path.clone());
                let expr = match expr_op {
                    "lower" => crate::index2::expr::IndexExpr::Lower(Box::new(base)),
                    "upper" => crate::index2::expr::IndexExpr::Upper(Box::new(base)),
                    "trim" => crate::index2::expr::IndexExpr::Trim(Box::new(base)),
                    "length" => crate::index2::expr::IndexExpr::Length(Box::new(base)),
                    _ => {
                        return Err(shamir_storage::error::DbError::Internal(format!(
                            "unknown functional_op: {expr_op}"
                        )))
                    }
                };
                let kind = IndexKind::Functional(Box::new(FunctionalConfig { expr: expr.clone() }));
                let desc = IndexDescriptor::new(
                    id,
                    &op.create_index,
                    name_key,
                    interned_paths.clone(),
                    kind.clone(),
                );
                let backend: Arc<dyn IndexBackend> =
                    Arc::new(crate::index2::functional_backend::FunctionalBackend::new(
                        desc,
                        expr,
                        Arc::clone(self.info_store()),
                    ));
                (kind, backend)
            }
            "vector" => {
                let dim = op.vector_dim.unwrap_or(384);
                let metric = match op.vector_metric.as_deref() {
                    Some("l2") => VectorMetric::L2,
                    Some("dot") => VectorMetric::Dot,
                    _ => VectorMetric::Cosine,
                };
                let kind = IndexKind::Vector(Box::new(VectorConfig {
                    dim,
                    metric,
                    backend: VectorBackendRef::InProcessHnsw {
                        ef_construct: 200,
                        m: 16,
                    },
                }));
                let desc = IndexDescriptor::new(
                    id,
                    &op.create_index,
                    name_key,
                    interned_paths.clone(),
                    kind.clone(),
                );
                let adapter = Arc::new(crate::index2::vector::hnsw_adapter::HnswAdapter::new(
                    dim,
                    metric,
                    crate::index2::vector::hnsw_adapter::HnswConfig {
                        max_elements: 100_000,
                        m: 16,
                        ef_construction: 200,
                        ef_search: 50,
                        ..Default::default()
                    },
                ));
                let backend: Arc<dyn IndexBackend> = Arc::new(
                    crate::index2::vector::VectorBackend::new(desc, first_path, adapter),
                );
                (kind, backend)
            }
            _ => {
                return Err(shamir_storage::error::DbError::Internal(format!(
                    "unknown index_type: {index_type}"
                )))
            }
        };

        self.index2_registry
            .insert(backend)
            .await
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;

        crate::index2::persistence::save_index2_metadata(&self.index2_registry, &self.info_store)
            .await?;

        Ok(())
    }

    pub async fn create_index(&self, name: &str, paths: &[&str]) -> DbResult<()> {
        let index_def = self.build_index_definition(name, paths).await?;
        self.index_manager.create_index(index_def).await
    }

    /// Create a unique index on specified paths.
    ///
    /// # Arguments
    /// * `name` - Index name (will be interned)
    /// * `paths` - Field paths, e.g. `["email"]`
    ///
    /// # Errors
    /// Returns `DbError::UniqueIndexCreationFailed` if duplicate values exist.
    pub async fn create_unique_index(&self, name: &str, paths: &[&str]) -> DbResult<()> {
        let index_def = self.build_index_definition(name, paths).await?;
        self.index_manager.create_unique_index(index_def).await
    }

    /// Drop a regular index by name.
    ///
    /// # Returns
    /// `true` if index existed and was removed, `false` if not found.
    pub async fn drop_index(&self, name: &str) -> DbResult<bool> {
        let name_id = self.intern_string(name).await?;
        self.index_manager.drop_index(name_id).await
    }

    /// Drop a unique index by name.
    ///
    /// # Returns
    /// `true` if index existed and was removed, `false` if not found.
    pub async fn drop_unique_index(&self, name: &str) -> DbResult<bool> {
        let name_id = self.intern_string(name).await?;
        self.index_manager.drop_unique_index(name_id).await
    }

    /// Look up records by index value.
    ///
    /// # Arguments
    /// * `name` - Index name
    /// * `values` - Values to search for (must match index paths count)
    ///
    /// # Returns
    /// Set of RecordIds matching the index values.
    pub async fn lookup_by_index(
        &self,
        name: &str,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<RecordId>> {
        let name_id = self.intern_string(name).await?;
        self.index_manager.lookup_by_index(name_id, values).await
    }

    /// Check if a regular index exists.
    ///
    /// Note: This method is async because it may need to load the interner.
    pub async fn index_exists(&self, name: &str) -> bool {
        // Try to get interned ID; if not interned, index doesn't exist
        if let Ok(interner) = self.interner.get().await {
            if let Some(key) = interner.get_ind(name) {
                return self.index_manager.index_exists(key.id());
            }
        }
        false
    }

    /// Check if a unique index exists.
    ///
    /// Note: This method is async because it may need to load the interner.
    pub async fn unique_index_exists(&self, name: &str) -> bool {
        if let Ok(interner) = self.interner.get().await {
            if let Some(key) = interner.get_ind(name) {
                return self.index_manager.unique_index_exists(key.id());
            }
        }
        false
    }

    // ============================================================================
    // Internal helpers
    // ============================================================================

    /// Intern a single string, returning its u64 ID.
    async fn intern_string(&self, s: &str) -> DbResult<u64> {
        let interner = self.interner.get().await?;
        match interner.touch_ind(s) {
            Ok(TouchInd::New(key)) | Ok(TouchInd::Exists(key)) => Ok(key.id()),
            Err(e) => Err(shamir_storage::error::DbError::Codec(e.to_string())),
        }
    }

    /// Intern a path string like "user.address.city" into Vec<u64>.
    async fn intern_path(&self, path: &str) -> DbResult<Vec<u64>> {
        let interner = self.interner.get().await?;
        let mut result = Vec::new();

        for component in path.split('.') {
            let id = match interner.touch_ind(component) {
                Ok(TouchInd::New(key)) | Ok(TouchInd::Exists(key)) => key.id(),
                Err(e) => return Err(shamir_storage::error::DbError::Codec(e.to_string())),
            };
            result.push(id);
        }

        Ok(result)
    }

    /// Build IndexDefinition from string name and paths.
    async fn build_index_definition(
        &self,
        name: &str,
        paths: &[&str],
    ) -> DbResult<IndexDefinition> {
        let name_id = self.intern_string(name).await?;

        let mut interned_paths = Vec::with_capacity(paths.len());
        for path in paths {
            let path_components = self.intern_path(path).await?;
            interned_paths.push(IndexInfoItem::new(path_components));
        }

        Ok(IndexDefinition::new(name_id, interned_paths))
    }
}

/// Parse a DSL tokenizer spec string into a [`TokenizerKind`].
///
/// DSL names:
///   - `None` / `"whitespace"` / unknown → `Whitespace`
///   - `"unicode"` → `Unicode`
///   - `"ngram"` → `Ngram { n: 3 }` (default trigram)
///   - `"ngram2"` .. `"ngram9"` → `Ngram { n: <digit> }`
///   - `"stemmed_<lang>"` → `Full { <lang>, stopwords=true, stem=true }`
///     (falls back to `Whitespace` if the language suffix is unknown)
pub(crate) fn fts_tokenizer_from_dsl(spec: Option<&str>) -> crate::index2::kind::TokenizerKind {
    use crate::index2::kind::{StemLanguage, TokenizerKind};

    match spec {
        Some("unicode") => TokenizerKind::Unicode,
        Some("ngram") => TokenizerKind::Ngram { n: 3 },
        Some(s) if s.starts_with("ngram") => {
            let digits = &s["ngram".len()..];
            let n: u8 = digits.parse().unwrap_or(3);
            TokenizerKind::Ngram { n: n.max(1) }
        }
        Some(s) if s.starts_with("stemmed_") => {
            let rest = &s["stemmed_".len()..];
            match StemLanguage::from_dsl(rest) {
                Some(lang) => TokenizerKind::Full {
                    language: lang,
                    stopwords: true,
                    stem: true,
                },
                None => TokenizerKind::Whitespace,
            }
        }
        _ => TokenizerKind::Whitespace,
    }
}
