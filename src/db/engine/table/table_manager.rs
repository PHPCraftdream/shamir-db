use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;

use super::interner_manager::InternerManager;
use super::record_counter::RecordCounter;
use super::table::Table;
use crate::core::interner::TouchInd;
use crate::db::engine::index::index_definition::IndexDefinition;
use crate::db::engine::index::index_info_item::IndexInfoItem;
use crate::db::engine::index::index_manager::IndexManager;
use crate::db::query::filter::eval::{compile_filter, FilterCallback};
use crate::db::query::filter::eval_context::FilterContext;
use crate::db::query::filter::Filter;
use crate::db::query::read::exec;
use crate::db::query::read::{PaginationInfo, QueryResult, QueryStats, ReadQuery};
use crate::db::storage::types::Store;
use crate::db::DbResult;
use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;

pub struct TableManager {
    name: String,
    table: Arc<Table>,
    interner: InternerManager,
    counter: Arc<RecordCounter>,
    index_manager: IndexManager,
}

impl Clone for TableManager {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            table: Arc::clone(&self.table),
            interner: self.interner.clone(),
            counter: Arc::clone(&self.counter),
            index_manager: self.index_manager.clone(),
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
        let table = Table::new(data_store);

        Ok(Self {
            name,
            table: Arc::new(table),
            interner,
            counter,
            index_manager,
        })
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
        Self {
            name,
            table: Arc::new(table),
            interner,
            counter,
            index_manager,
        }
    }

    #[cfg(test)]
    pub fn table(&self) -> &Table {
        &self.table
    }

    #[cfg(test)]
    pub fn interner(&self) -> &InternerManager {
        &self.interner
    }

    #[cfg(test)]
    pub fn counter(&self) -> &Arc<RecordCounter> {
        &self.counter
    }

    #[cfg(test)]
    pub fn index_manager(&self) -> &IndexManager {
        &self.index_manager
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

        Ok(id)
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
        } else if let Some(old) = old_value {
            self.index_manager
                .on_record_updated(&id, &old, value)
                .await?;
            self.index_manager
                .on_record_updated_unique(&id, &old, value)
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
    // Read query execution
    // ============================================================================

    /// Execute a full read query pipeline via streaming.
    ///
    /// Three strategies depending on query shape:
    ///
    /// 1. **Streaming** (no ORDER BY, GROUP BY, DISTINCT, aggregates, count_total):
    ///    projects on-the-fly, fetches `limit + 1` for accurate `has_next`,
    ///    early termination. Memory ≈ page_size.
    ///
    /// 2. **Counting** (count_total + simple SELECT — no ORDER BY, GROUP BY,
    ///    DISTINCT, aggregates): streams all records counting matched total,
    ///    but only keeps the requested page in memory. Memory ≈ page_size.
    ///
    /// 3. **Collecting** (ORDER BY, GROUP BY, DISTINCT, aggregates): must
    ///    accumulate records — raw InnerValues for GROUP BY / aggregates,
    ///    projected JSON for ORDER BY / DISTINCT.
    pub async fn read(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
    ) -> DbResult<QueryResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner.get().await?;

        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);
        let has_order = query.order_by.is_some();
        let has_distinct = query.select.distinct;

        // Compile WHERE filter once (if present)
        let filter_cb: Option<Box<dyn FilterCallback>> =
            query.r#where.as_ref().map(|f| compile_filter(f, interner));

        let needs_full_collect = has_group_by || has_agg || has_order || has_distinct;

        if needs_full_collect {
            // Path 3: must accumulate for GROUP BY / ORDER BY / DISTINCT / aggregates
            self.read_collecting(query, ctx, interner, filter_cb.as_deref(), batch_size, start)
                .await
        } else if query.count_total {
            // Path 2: count all matched, keep only the page
            self.read_counting(query, interner, filter_cb.as_deref(), ctx, batch_size, start)
                .await
        } else {
            // Path 1: pure streaming, early termination
            self.read_streaming(query, interner, filter_cb.as_deref(), ctx, batch_size, start)
                .await
        }
    }

    /// Collecting path: streams batches, accumulates what's needed, then applies
    /// GROUP BY / aggregates / ORDER BY / DISTINCT / PAGINATION.
    ///
    /// For GROUP BY / aggregates — accumulates raw InnerValues (needed for
    /// field extraction). For plain SELECT + ORDER BY / DISTINCT — accumulates
    /// already-projected JSON values (smaller footprint than raw records).
    async fn read_collecting(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &crate::core::interner::Interner,
        filter_cb: Option<&dyn FilterCallback>,
        batch_size: usize,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);
        let needs_raw = has_group_by || has_agg;

        let stream = self.table.list_stream(batch_size);
        futures::pin_mut!(stream);

        let mut records_scanned: u64 = 0;

        // Two accumulation modes — raw InnerValues or projected JSON
        let mut raw_acc: Vec<(RecordId, InnerValue)> = Vec::new();
        let mut json_acc: Vec<serde_json::Value> = Vec::new();
        let proj = if !needs_raw {
            Some(exec::SelectProjection::new(&query.select, interner))
        } else {
            None
        };

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            records_scanned += batch.len() as u64;
            for (id, record) in batch {
                let passes = match filter_cb {
                    Some(cb) => cb.matches(&record, ctx),
                    None => true,
                };
                if passes {
                    if needs_raw {
                        raw_acc.push((id, record));
                    } else {
                        json_acc.push(proj.as_ref().unwrap().project(&record, interner));
                    }
                }
            }
        }

        let mut result = if has_group_by {
            let group_by = query.group_by.as_ref().unwrap();
            exec::apply_group_by(&raw_acc, group_by, &query.select, interner, ctx)
        } else if has_agg {
            exec::apply_aggregate_all(&raw_acc, &query.select, interner)
        } else {
            json_acc
        };

        if query.select.distinct {
            result = exec::apply_distinct(result);
        }
        if let Some(ref order_by) = query.order_by {
            exec::apply_order_by(&mut result, order_by);
        }

        let (records, pagination) =
            exec::apply_pagination(result, &query.pagination, query.count_total);

        let elapsed = start.elapsed();
        let records_returned = records.len() as u64;

        Ok(QueryResult {
            records,
            stats: Some(QueryStats {
                index_used: None,
                records_scanned,
                records_returned,
                execution_time_us: elapsed.as_micros() as u64,
            }),
            pagination,
        })
    }

    /// Counting path: streams all records, counts total matched, but only
    /// keeps the requested page in memory. Memory ≈ page_size (not total).
    ///
    /// Used when `count_total = true` but no ORDER BY / GROUP BY / DISTINCT /
    /// aggregates — i.e. the order is natural (insertion order) so we can
    /// paginate on-the-fly while still counting everything.
    async fn read_counting(
        &self,
        query: &ReadQuery,
        interner: &crate::core::interner::Interner,
        filter_cb: Option<&dyn FilterCallback>,
        ctx: &FilterContext<'_>,
        batch_size: usize,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let (skip, take) = query.pagination.resolve();
        let skip = skip as usize;
        let limit = take.map(|t| t as usize);

        let proj = exec::SelectProjection::new(&query.select, interner);

        let stream = self.table.list_stream(batch_size);
        futures::pin_mut!(stream);

        let mut records_scanned: u64 = 0;
        let mut matched_total: u64 = 0;
        let mut result: Vec<serde_json::Value> = Vec::new();

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            records_scanned += batch.len() as u64;

            for (_, record) in &batch {
                let passes = match filter_cb {
                    Some(cb) => cb.matches(record, ctx),
                    None => true,
                };
                if !passes {
                    continue;
                }

                let idx = matched_total as usize;
                matched_total += 1;

                // Only project and keep records that fall within the page
                if idx >= skip {
                    if let Some(lim) = limit {
                        if idx < skip + lim {
                            result.push(proj.project(record, interner));
                        }
                        // Beyond the page — still count, but don't store
                    } else {
                        // No limit — keep everything from skip onwards
                        result.push(proj.project(record, interner));
                    }
                }
            }
        }

        let elapsed = start.elapsed();
        let records_returned = result.len() as u64;

        let pagination = Some(PaginationInfo::compute(
            &query.pagination,
            Some(matched_total),
        ));

        Ok(QueryResult {
            records: result,
            stats: Some(QueryStats {
                index_used: None,
                records_scanned,
                records_returned,
                execution_time_us: elapsed.as_micros() as u64,
            }),
            pagination,
        })
    }

    /// Streaming path: SELECT + PAGINATION only (no ORDER BY, GROUP BY, DISTINCT,
    /// aggregates, count_total). Projects on-the-fly, fetches up to `limit + 1`
    /// to determine `has_next` accurately, then stops. Memory ≈ page_size.
    async fn read_streaming(
        &self,
        query: &ReadQuery,
        interner: &crate::core::interner::Interner,
        filter_cb: Option<&dyn FilterCallback>,
        ctx: &FilterContext<'_>,
        batch_size: usize,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let (skip, take) = query.pagination.resolve();
        let skip = skip as usize;
        let limit = take.map(|t| t as usize);

        let proj = exec::SelectProjection::new(&query.select, interner);

        let stream = self.table.list_stream(batch_size);
        futures::pin_mut!(stream);

        let mut records_scanned: u64 = 0;
        let mut skipped: usize = 0;
        let mut result: Vec<serde_json::Value> = Vec::new();
        let mut has_next = false;
        let mut done = false;

        while let Some(batch_result) = stream.next().await {
            if done {
                break;
            }
            let batch = batch_result?;
            records_scanned += batch.len() as u64;

            for (_, record) in &batch {
                let passes = match filter_cb {
                    Some(cb) => cb.matches(record, ctx),
                    None => true,
                };
                if !passes {
                    continue;
                }

                if skipped < skip {
                    skipped += 1;
                    continue;
                }

                if let Some(lim) = limit {
                    if result.len() >= lim {
                        // This is the limit+1 record — confirms has_next
                        has_next = true;
                        done = true;
                        break;
                    }
                }

                result.push(proj.project(record, interner));
            }
        }

        let elapsed = start.elapsed();
        let records_returned = result.len() as u64;

        let pagination = if query.pagination.is_none() {
            None
        } else {
            Some(
                PaginationInfo::compute(&query.pagination, None)
                    .with_has_next(has_next),
            )
        };

        Ok(QueryResult {
            records: result,
            stats: Some(QueryStats {
                index_used: None,
                records_scanned,
                records_returned,
                execution_time_us: elapsed.as_micros() as u64,
            }),
            pagination,
        })
    }

    // ============================================================================
    // Internal helpers
    // ============================================================================

    /// Intern a single string, returning its u64 ID.
    async fn intern_string(&self, s: &str) -> DbResult<u64> {
        let interner = self.interner.get().await?;
        match interner.touch_ind(s) {
            Ok(TouchInd::New(key)) | Ok(TouchInd::Exists(key)) => Ok(key.id()),
            Err(e) => Err(crate::db::DbError::Codec(e.to_string())),
        }
    }

    /// Intern a path string like "user.address.city" into Vec<u64>.
    async fn intern_path(&self, path: &str) -> DbResult<Vec<u64>> {
        let interner = self.interner.get().await?;
        let mut result = Vec::new();

        for component in path.split('.') {
            let id = match interner.touch_ind(component) {
                Ok(TouchInd::New(key)) | Ok(TouchInd::Exists(key)) => key.id(),
                Err(e) => return Err(crate::db::DbError::Codec(e.to_string())),
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
