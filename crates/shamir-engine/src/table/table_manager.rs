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
use crate::wal::WalManager;
use shamir_storage::error::DbResult;
use shamir_storage::storage_membuffer::MemBufferConfig;
use shamir_storage::types::Store;
use shamir_types::core::interner::TouchInd;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

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

    /// Borrow the table's sorted-index manager — used by the planner
    /// for range / order / min queries, and by DDL when a
    /// `create_index { sorted: true }` op lands.
    pub fn sorted_indexes(&self) -> &SortedIndexManager {
        &self.sorted_indexes
    }

    async fn index2_on_insert(&self, rid: &RecordId, rec: &InnerValue) {
        for backend in self.index2_registry.all_backends().await {
            let _ = backend.on_insert(*rid, rec).await;
        }
    }

    async fn index2_on_update(&self, rid: &RecordId, old: &InnerValue, new: &InnerValue) {
        for backend in self.index2_registry.all_backends().await {
            let _ = backend.on_update(*rid, old, new).await;
        }
    }

    async fn index2_on_delete(&self, rid: &RecordId, rec: &InnerValue) {
        for backend in self.index2_registry.all_backends().await {
            let _ = backend.on_delete(*rid, rec).await;
        }
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

        // 2. Write to table
        let id = self.table.insert(value).await?;
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

        // 2. One batched data-store write.
        let ids = self.table.insert_many(values).await?;

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
                crate::wal::WalManager::ops_record_created(&ids),
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
        let removed = self.table.delete(id).await?;
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

        // 2. Write to table
        let created = self.table.set(id, value).await?;

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

    /// Get a record by RecordId
    pub async fn get(&self, id: RecordId) -> DbResult<InnerValue> {
        self.table.get(id).await
    }

    // ============================================================================
    // Migration support — index2 cutover
    // ============================================================================

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
    /// TableManager's data_store and calling `on_batch_insert` for each
    /// batch on every registered backend.
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
                backend.on_batch_insert(&items).await.map_err(|e| {
                    shamir_storage::error::DbError::Internal(format!(
                        "bulk_populate_index2 on_batch_insert failed: {}",
                        e
                    ))
                })?;
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

        let (kind, backend): (IndexKind, Arc<dyn IndexBackend>) = match index_type {
            "fts" => {
                // DSL names for fts_tokenizer:
                //   "whitespace"      → plain whitespace split
                //   "unicode"         → unicode-aware split
                //   "stemmed_en"      → Full { English, stopwords=true, stem=true }
                //   "stemmed_ru"      → Full { Russian, stopwords=true, stem=true }
                //   "stemmed_english" / "stemmed_russian" — aliases
                let tok = match op.fts_tokenizer.as_deref() {
                    Some("unicode") => TokenizerKind::Unicode,
                    Some("stemmed_en" | "stemmed_english") => TokenizerKind::Full {
                        language: StemLanguage::English,
                        stopwords: true,
                        stem: true,
                    },
                    Some("stemmed_ru" | "stemmed_russian") => TokenizerKind::Full {
                        language: StemLanguage::Russian,
                        stopwords: true,
                        stem: true,
                    },
                    _ => TokenizerKind::Whitespace,
                };
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
