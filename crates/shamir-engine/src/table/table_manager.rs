use std::collections::BTreeSet;
use std::sync::Arc;

use futures::StreamExt;

use super::interner_manager::InternerManager;
use super::record_counter::RecordCounter;
use super::table::Table;
use shamir_types::core::interner::TouchInd;
use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;
use crate::index::index_manager::IndexManager;
use crate::index::sorted_index_manager::SortedIndexManager;
use crate::query::filter::eval::{compile_filter, FilterCallback};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::Filter;
use crate::wal::WalManager;
use shamir_storage::types::Store;
use shamir_storage::error::DbResult;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

pub struct TableManager {
    name: String,
    table: Arc<Table>,
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
            interner: self.interner.clone(),
            counter: Arc::clone(&self.counter),
            index_manager: self.index_manager.clone(),
            sorted_indexes: self.sorted_indexes.clone(),
            wal: Arc::clone(&self.wal),
            write_counter: Arc::clone(&self.write_counter),
            verify_running: Arc::clone(&self.verify_running),
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
            interner,
            counter,
            index_manager,
            sorted_indexes,
            wal,
            write_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            verify_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };

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
        let info_store: Arc<dyn Store> = Arc::new(
            shamir_storage::storage_in_memory::InMemoryStore::new(),
        );
        // Construct synchronously: SortedIndexManager::new() is async
        // but the empty-state path doesn't await any real work.
        let sorted_indexes = futures::executor::block_on(
            SortedIndexManager::new(info_store.clone()),
        )
        .expect("sorted index manager init for test");
        let wal = Arc::new(WalManager::new(info_store));
        Self {
            name,
            table: Arc::new(table),
            interner,
            counter,
            index_manager,
            sorted_indexes,
            wal,
            write_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            verify_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        let crossed = prev / AUTO_VERIFY_EVERY_N_WRITES
            != next / AUTO_VERIFY_EVERY_N_WRITES;
        if !crossed {
            return;
        }
        // Single-flight: skip if another verify is in flight.
        if self
            .verify_running
            .compare_exchange(
                false,
                true,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
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
                Err(e) => log::warn!(
                    "Background verify on '{}' failed: {}",
                    self_clone.name(),
                    e,
                ),
            }
            self_clone
                .verify_running
                .store(false, Ordering::Release);
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

    /// Borrow the table's sorted-index manager — used by the planner
    /// for range / order / min queries, and by DDL when a
    /// `create_index { sorted: true }` op lands.
    pub fn sorted_indexes(&self) -> &SortedIndexManager {
        &self.sorted_indexes
    }

    /// Register a new sorted (B-tree-by-value) index over a single
    /// scalar field, then backfill it from existing records.
    pub async fn create_sorted_index(
        &self,
        index_name: &str,
        field_path: &[&str],
    ) -> DbResult<()> {
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
    pub async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
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
            }
        }
        Ok(removed)
    }

    /// Set a record by RecordId - creates if not exists, updates if exists (with counter and index update)
    ///
    /// Validates unique indexes BEFORE write, returns error if constraint violated.
    pub async fn set(&self, id: RecordId, value: &InnerValue) -> DbResult<bool> {
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
