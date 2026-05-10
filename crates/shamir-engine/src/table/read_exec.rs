//! Read query execution on TableManager.
//!
//! Implements read(), index scan planning, and read execution strategies
//! (collecting, counting, streaming) for TableManager.

use std::time::Instant;

use futures::StreamExt;

use shamir_types::core::interner::{Interner, InternerKey};
use crate::query::filter::eval::{compile_filter, filter_value_to_inner, intern_field_path, FilterCallback};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::{exec, PaginationInfo, QueryResult, QueryStats, ReadQuery, SelectItem};
use shamir_types::core::sort_codec;
use shamir_storage::error::DbResult;
use shamir_types::types::common::new_set;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use super::table_manager::TableManager;

/// Encode a scalar `FilterValue` with `sort_codec` so range bounds
/// can be compared to physical sorted-index keys. Returns `None` for
/// values that can't be indexed (NaN floats, non-scalars).
fn encode_filter_value_for_sort(value: &FilterValue) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    match value {
        FilterValue::Int(i) => sort_codec::encode_i64(&mut buf, *i),
        FilterValue::Float(f) => sort_codec::encode_f64(&mut buf, *f).ok()?,
        FilterValue::String(s) => sort_codec::encode_str(&mut buf, s),
        FilterValue::Bool(b) => sort_codec::encode_bool(&mut buf, *b),
        FilterValue::Null => sort_codec::encode_null(&mut buf),
        FilterValue::Binary(b) => sort_codec::encode_bytes(&mut buf, b),
        _ => return None,
    }
    Some(buf)
}

impl TableManager {
    // ============================================================================
    // Index scan planning
    // ============================================================================

    /// Try to find an index that can satisfy (part of) the filter.
    ///
    /// Returns `Some((index_name_interned, lookup_value_sets, residual_filter))`:
    /// - `lookup_value_sets` — one set per lookup (Eq -> 1 set, In -> N sets)
    /// - Each set is passed to `lookup_by_index` separately, results are unioned
    pub fn try_plan_index_scan(
        &self,
        filter: &Filter,
        interner: &Interner,
    ) -> Option<(u64, Vec<Vec<InnerValue>>, Option<Filter>)> {
        let idx_mgr = self.index_manager_ref();
        if !idx_mgr.has_indexes() {
            return None;
        }

        match filter {
            // Simple Eq: one lookup
            Filter::Eq { field, value } | Filter::FieldEq { field, value } => {
                let inner_val = filter_value_to_inner(value)?;
                let field_path = intern_field_path(field, interner)?;
                let idx = self.find_single_field_index(&field_path)?;
                Some((idx, vec![vec![inner_val]], None))
            }

            // In: multiple lookups, union results
            Filter::In { field, values } => {
                let field_path = intern_field_path(field, interner)?;
                let idx = self.find_single_field_index(&field_path)?;

                let mut sets = Vec::with_capacity(values.len());
                for v in values {
                    let inner = filter_value_to_inner(v)?;
                    sets.push(vec![inner]);
                }
                if sets.is_empty() {
                    return None;
                }
                Some((idx, sets, None))
            }

            // And: extract Eq/In conditions, try to match indexes
            Filter::And { filters } => {
                self.try_plan_and_index_scan(filters, interner)
            }

            _ => None,
        }
    }

    /// Find a single-field index whose path matches `field_path`.
    pub fn find_single_field_index(&self, field_path: &[u64]) -> Option<u64> {
        for def in self.index_manager_ref().iter_indexes() {
            if def.paths.len() == 1 && def.paths[0].path == field_path {
                return Some(def.name_interned);
            }
        }
        None
    }

    /// Try to plan an index scan from an And filter.
    fn try_plan_and_index_scan(
        &self,
        filters: &[Filter],
        interner: &Interner,
    ) -> Option<(u64, Vec<Vec<InnerValue>>, Option<Filter>)> {
        // Collect indexable conditions: (filter_index, field_path, lookup_sets)
        // Eq -> 1 set, In -> N sets
        struct IndexableItem {
            filter_idx: usize,
            field_path: Vec<u64>,
            lookup_sets: Vec<Vec<InnerValue>>,
        }

        let mut items: Vec<IndexableItem> = Vec::new();
        for (i, f) in filters.iter().enumerate() {
            match f {
                Filter::Eq { field, value } | Filter::FieldEq { field, value } => {
                    if let Some(inner) = filter_value_to_inner(value) {
                        if let Some(fp) = intern_field_path(field, interner) {
                            items.push(IndexableItem {
                                filter_idx: i,
                                field_path: fp,
                                lookup_sets: vec![vec![inner]],
                            });
                        }
                    }
                }
                Filter::In { field, values } => {
                    if let Some(fp) = intern_field_path(field, interner) {
                        let mut sets = Vec::new();
                        let mut all_literal = true;
                        for v in values {
                            if let Some(inner) = filter_value_to_inner(v) {
                                sets.push(vec![inner]);
                            } else {
                                all_literal = false;
                                break;
                            }
                        }
                        if all_literal && !sets.is_empty() {
                            items.push(IndexableItem {
                                filter_idx: i,
                                field_path: fp,
                                lookup_sets: sets,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        if items.is_empty() {
            return None;
        }

        let idx_mgr = self.index_manager_ref();

        // Try composite indexes first (Eq-only, each path covered by exactly one Eq)
        for def in idx_mgr.iter_indexes() {
            if def.paths.len() > 1 {
                let mut lookup_values = Vec::with_capacity(def.paths.len());
                let mut consumed = Vec::new();
                let mut all_matched = true;

                for idx_path in &def.paths {
                    if let Some(item) = items.iter().find(|it| {
                        it.field_path == idx_path.path && it.lookup_sets.len() == 1
                    }) {
                        lookup_values.push(item.lookup_sets[0][0].clone());
                        consumed.push(item.filter_idx);
                    } else {
                        all_matched = false;
                        break;
                    }
                }

                if all_matched {
                    let residual = Self::build_residual(filters, &consumed);
                    return Some((def.name_interned, vec![lookup_values], residual));
                }
            }
        }

        // Try single-field indexes (Eq or In)
        for def in idx_mgr.iter_indexes() {
            if def.paths.len() == 1 {
                if let Some(item) = items.iter().find(|it| it.field_path == def.paths[0].path) {
                    let consumed = vec![item.filter_idx];
                    let residual = Self::build_residual(filters, &consumed);
                    return Some((def.name_interned, item.lookup_sets.clone(), residual));
                }
            }
        }

        None
    }

    /// Build residual filter from And children, excluding consumed indices.
    fn build_residual(filters: &[Filter], consumed: &[usize]) -> Option<Filter> {
        let remaining: Vec<Filter> = filters
            .iter()
            .enumerate()
            .filter(|(i, _)| !consumed.contains(i))
            .map(|(_, f)| f.clone())
            .collect();

        match remaining.len() {
            0 => None,
            1 => Some(remaining.into_iter().next().unwrap()),
            _ => Some(Filter::And {
                filters: remaining,
            }),
        }
    }

    // ============================================================================
    // Read query execution
    // ============================================================================

    /// Execute a read query pipeline.
    ///
    /// Tries index scan first if a suitable index exists for the WHERE clause.
    /// Falls back to streaming scan otherwise.
    ///
    /// Streaming scan has three sub-strategies:
    /// 1. **Streaming** — early termination, memory ~ page_size
    /// 2. **Counting** — count_total without ORDER BY, memory ~ page_size
    /// 3. **Collecting** — ORDER BY / GROUP BY / DISTINCT / aggregates
    pub async fn read(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
    ) -> DbResult<QueryResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // Opt #2 (1000×-class): `SELECT count(*) FROM table` (no WHERE,
        // no GROUP BY, no DISTINCT, no ORDER BY, no pagination, no
        // count_total flag) is answered directly from the persistent
        // record counter — O(1), no scan, no allocation. Previously
        // it materialised every record just to call `.len()` on the
        // result vector.
        if query.r#where.is_none()
            && query.group_by.is_none()
            && query.order_by.is_none()
            && !query.select.distinct
            && !query.count_total
            && query.pagination.is_none()
            && query.select.items.len() == 1
        {
            // Q1 (1000×-class for MIN-only aggregate): if SELECT is
            // exactly `min(field)` and there's a sorted index on
            // `field`, we walk the index to its first key and return
            // a single-record answer. O(log n) instead of full scan.
            // MAX is symmetric but needs reverse iter on Store
            // (Opt R) — not wired yet, falls through to full scan.
            if let SelectItem::Aggregate {
                func: shamir_query_types::read::AggFunc::Min,
                field: shamir_query_types::read::AggregateField::Field(path),
                alias,
                ..
            } = &query.select.items[0]
            {
                if let Some(field_path) = intern_field_path(path, interner) {
                    if let Some(def) = self.sorted_indexes().find_by_field(&field_path) {
                        if let Some(id) =
                            self.sorted_indexes().lookup_min(def.name_interned).await?
                        {
                            // Load the record and extract the field value.
                            let record = self.get(id).await?;
                            let val = crate::query::filter::eval::resolve_field(
                                &record, &field_path,
                            );
                            let json_val = match val {
                                Some(v) => shamir_types::codecs::interned::inner_to_json_value(
                                    &v, interner,
                                ),
                                None => serde_json::Value::Null,
                            };
                            let key = alias
                                .as_deref()
                                .unwrap_or_else(|| {
                                    path.last().map(|s| s.as_str()).unwrap_or("min")
                                })
                                .to_string();
                            let mut obj = serde_json::Map::new();
                            obj.insert(key, json_val);
                            return Ok(QueryResult {
                                records: vec![serde_json::Value::Object(obj)],
                                stats: Some(QueryStats {
                                    index_used: Some(format!(
                                        "sorted_idx_{}_min",
                                        def.name_interned
                                    )),
                                    records_scanned: 1,
                                    records_returned: 1,
                                    execution_time_us: start.elapsed().as_micros() as u64,
                                }),
                                pagination: None,
                            });
                        }
                    }
                }
            }

            if let SelectItem::CountAll { alias } = &query.select.items[0] {
                let count: u64 = self.counter().get().await?;
                let key = alias.as_deref().unwrap_or("count").to_string();
                let mut obj = serde_json::Map::new();
                obj.insert(key, serde_json::Value::Number(count.into()));
                return Ok(QueryResult {
                    records: vec![serde_json::Value::Object(obj)],
                    stats: Some(QueryStats {
                        index_used: Some("__record_counter__".to_string()),
                        records_scanned: 0,
                        records_returned: 1,
                        execution_time_us: start.elapsed().as_micros() as u64,
                    }),
                    pagination: None,
                });
            }
        }

        // Try index scan first
        if let Some(ref filter) = query.r#where {
            if let Some((idx_name, lookup_sets, residual)) =
                self.try_plan_index_scan(filter, interner)
            {
                // Opt #2.5 (1000×-class): `count(*) WHERE indexed_eq`
                // collapses to `BTreeSet::len()` of the index lookup —
                // no record materialisation. Eligible when:
                //   - the WHERE is fully covered by the index (no residual)
                //   - select is exactly one CountAll item
                //   - no group_by, order_by, distinct, count_total, pagination
                if residual.is_none()
                    && query.group_by.is_none()
                    && query.order_by.is_none()
                    && !query.select.distinct
                    && !query.count_total
                    && query.pagination.is_none()
                    && query.select.items.len() == 1
                {
                    if let SelectItem::CountAll { alias } = &query.select.items[0] {
                        let mut total: u64 = 0;
                        for values in &lookup_sets {
                            let ids = self
                                .index_manager_ref()
                                .lookup_by_index(idx_name, values)
                                .await?;
                            total += ids.len() as u64;
                        }
                        let key = alias.as_deref().unwrap_or("count").to_string();
                        let mut obj = serde_json::Map::new();
                        obj.insert(key, serde_json::Value::Number(total.into()));
                        return Ok(QueryResult {
                            records: vec![serde_json::Value::Object(obj)],
                            stats: Some(QueryStats {
                                index_used: Some(format!("idx_{idx_name}")),
                                records_scanned: total,
                                records_returned: 1,
                                execution_time_us: start.elapsed().as_micros() as u64,
                            }),
                            pagination: None,
                        });
                    }
                }

                return self
                    .read_index_scan(query, ctx, interner, idx_name, &lookup_sets, residual.as_ref(), start)
                    .await;
            }
        }

        // Sorted-index plan (range / Gte / Lte / Between). Only kicks
        // in for the supported filter shapes — falls through to scan
        // otherwise.
        if let Some(ref filter) = query.r#where {
            if let Some((idx_name, lo, hi, residual)) =
                self.try_plan_sorted_index_scan(filter, interner)
            {
                return self
                    .read_sorted_index_scan(
                        query,
                        ctx,
                        interner,
                        idx_name,
                        lo.as_deref(),
                        hi.as_deref(),
                        residual.as_ref(),
                        start,
                    )
                    .await;
            }
        }

        // Fall back to full scan
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);
        let has_order = query.order_by.is_some();
        let has_distinct = query.select.distinct;

        let filter_cb: Option<Box<dyn FilterCallback>> =
            query.r#where.as_ref().map(|f| compile_filter(f, interner));

        let needs_full_collect = has_group_by || has_agg || has_order || has_distinct;

        if needs_full_collect {
            self.read_collecting(query, ctx, interner, filter_cb.as_deref(), batch_size, start)
                .await
        } else if query.count_total {
            self.read_counting(query, interner, filter_cb.as_deref(), ctx, batch_size, start)
                .await
        } else {
            self.read_streaming(query, interner, filter_cb.as_deref(), ctx, batch_size, start)
                .await
        }
    }

    /// Try to plan a sorted-index scan for the supported range
    /// filters (`Between`, `Gte`, `Lte`). Returns
    /// `(name_interned, lower_encoded, upper_encoded, residual)` or
    /// `None` if no sorted index applies.
    ///
    /// `lower_encoded` / `upper_encoded` are bytes produced by
    /// `sort_codec` — `None` for an open bound.
    ///
    /// Gt / Lt are intentionally NOT routed here yet — they need an
    /// "exclude exact-match boundary" trick that we'll add in a
    /// follow-up. They fall through to the full-scan path.
    pub fn try_plan_sorted_index_scan(
        &self,
        filter: &Filter,
        interner: &Interner,
    ) -> Option<(u64, Option<Vec<u8>>, Option<Vec<u8>>, Option<Filter>)> {
        let mgr = self.sorted_indexes();
        if !mgr.has_indexes() {
            return None;
        }

        match filter {
            Filter::Between { field, from, to } => {
                let field_path = intern_field_path(field, interner)?;
                let def = mgr.find_by_field(&field_path)?;
                let lo = encode_filter_value_for_sort(from)?;
                let hi = encode_filter_value_for_sort(to)?;
                Some((def.name_interned, Some(lo), Some(hi), None))
            }
            Filter::Gte { field, value } => {
                let field_path = intern_field_path(field, interner)?;
                let def = mgr.find_by_field(&field_path)?;
                let lo = encode_filter_value_for_sort(value)?;
                Some((def.name_interned, Some(lo), None, None))
            }
            Filter::Lte { field, value } => {
                let field_path = intern_field_path(field, interner)?;
                let def = mgr.find_by_field(&field_path)?;
                let hi = encode_filter_value_for_sort(value)?;
                Some((def.name_interned, None, Some(hi), None))
            }
            // Q2: strict-bound variants. We don't try to compute an
            // exclusive byte-suffix successor (encoding-dependent,
            // brittle); instead we use the inclusive Gte/Lte index
            // window and add an `Ne(value)` residual filter to
            // exclude the boundary at evaluation time. Cheap — the
            // boundary value typically yields at most a handful of
            // records to filter.
            Filter::Gt { field, value } => {
                let field_path = intern_field_path(field, interner)?;
                let def = mgr.find_by_field(&field_path)?;
                let lo = encode_filter_value_for_sort(value)?;
                let residual = Filter::Ne {
                    field: field.clone(),
                    value: value.clone(),
                };
                Some((def.name_interned, Some(lo), None, Some(residual)))
            }
            Filter::Lt { field, value } => {
                let field_path = intern_field_path(field, interner)?;
                let def = mgr.find_by_field(&field_path)?;
                let hi = encode_filter_value_for_sort(value)?;
                let residual = Filter::Ne {
                    field: field.clone(),
                    value: value.clone(),
                };
                Some((def.name_interned, None, Some(hi), Some(residual)))
            }
            _ => None,
        }
    }

    /// Scan a sorted index for a range of record_ids, then apply the
    /// usual read pipeline (residual filter, projection, group_by,
    /// aggregates, sort, paginate).
    async fn read_sorted_index_scan(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        index_name: u64,
        lower_encoded: Option<&[u8]>,
        upper_encoded: Option<&[u8]>,
        residual: Option<&Filter>,
        start: Instant,
    ) -> DbResult<QueryResult> {
        // 1. Lookup matching RecordIds from the sorted index.
        let record_ids = self
            .sorted_indexes()
            .lookup_range(index_name, lower_encoded, upper_encoded)
            .await?;

        // 2. Compile residual filter if present.
        let residual_cb: Option<Box<dyn FilterCallback>> =
            residual.map(|f| compile_filter(f, interner));

        // 3. Fetch records and apply residual filter.
        let mut matched: Vec<(RecordId, InnerValue)> =
            Vec::with_capacity(record_ids.len());
        for id in &record_ids {
            match self.get(*id).await {
                Ok(record) => {
                    let passes = match &residual_cb {
                        Some(cb) => cb.matches(&record, ctx),
                        None => true,
                    };
                    if passes {
                        matched.push((*id, record));
                    }
                }
                Err(shamir_storage::error::DbError::NotFound(_)) => continue,
                Err(e) => return Err(e),
            }
        }

        // 4. Re-use the same pipeline tail as the equality index path
        //    by calling the same projection / sort / paginate helpers.
        //    Inline the bits we need from `read_index_scan` body.
        let records_scanned = matched.len() as u64;

        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);

        let mut result = if has_group_by {
            let group_by = query.group_by.as_ref().unwrap();
            exec::apply_group_by(&matched, group_by, &query.select, interner, ctx)
        } else if has_agg {
            exec::apply_aggregate_all(&matched, &query.select, interner)
        } else {
            exec::apply_select(&matched, &query.select, interner)
        };

        if let Some(ref order_by) = query.order_by {
            exec::apply_order_by(&mut result, order_by);
        }

        let (paged, pagination) =
            exec::apply_pagination(result, &query.pagination, query.count_total);
        let records_returned = paged.len() as u64;

        Ok(QueryResult {
            records: paged,
            stats: Some(QueryStats {
                index_used: Some(format!("sorted_idx_{index_name}")),
                records_scanned,
                records_returned,
                execution_time_us: start.elapsed().as_micros() as u64,
            }),
            pagination,
        })
    }

    /// (helper)
    /// Encode a FilterValue scalar into sort-stable bytes. Returns
    /// None for values we can't index (NaN, Null, arrays, maps).
    /// kept inside this impl block as a free fn via `fn encode_*`
    /// below.
    ///
    /// Index scan path: fetch records by index, apply residual filter + pipeline.
    ///
    /// `lookup_sets` contains one or more value sets to look up.
    /// For Eq — one set. For In — one set per value. Results are unioned.
    async fn read_index_scan(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        index_name: u64,
        lookup_sets: &[Vec<InnerValue>],
        residual: Option<&Filter>,
        start: Instant,
    ) -> DbResult<QueryResult> {
        // 1. Lookup matching RecordIds from index (union across all sets)
        let mut record_ids = new_set::<RecordId>();
        for values in lookup_sets {
            let ids = self.index_manager_ref().lookup_by_index(index_name, values).await?;
            record_ids.extend(ids);
        }

        // 2. Compile residual filter if present
        let residual_cb: Option<Box<dyn FilterCallback>> =
            residual.map(|f| compile_filter(f, interner));

        // 3. Fetch records by ID and apply residual filter
        let mut matched: Vec<(RecordId, InnerValue)> = Vec::with_capacity(record_ids.len());
        for id in &record_ids {
            match self.get(*id).await {
                Ok(record) => {
                    let passes = match &residual_cb {
                        Some(cb) => cb.matches(&record, ctx),
                        None => true,
                    };
                    if passes {
                        matched.push((*id, record));
                    }
                }
                Err(shamir_storage::error::DbError::NotFound(_)) => continue, // stale index entry
                Err(e) => return Err(e),
            }
        }

        let records_scanned = matched.len() as u64;

        // 4. Apply the rest of the pipeline (same as collecting path)
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);

        let mut result = if has_group_by {
            let group_by = query.group_by.as_ref().unwrap();
            exec::apply_group_by(&matched, group_by, &query.select, interner, ctx)
        } else if has_agg {
            exec::apply_aggregate_all(&matched, &query.select, interner)
        } else {
            exec::apply_select(&matched, &query.select, interner)
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

        // Resolve index name for stats
        let index_name_str = interner
            .get_str(&InternerKey::new(index_name))
            .map(|k| k.as_str().to_string())
            .unwrap_or_else(|| index_name.to_string());

        Ok(QueryResult {
            records,
            stats: Some(QueryStats {
                index_used: Some(index_name_str),
                records_scanned,
                records_returned,
                execution_time_us: elapsed.as_micros() as u64,
            }),
            pagination,
        })
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
        interner: &Interner,
        filter_cb: Option<&dyn FilterCallback>,
        batch_size: usize,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);
        let needs_raw = has_group_by || has_agg;

        let stream = self.list_stream(batch_size);
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
    /// keeps the requested page in memory. Memory ~ page_size (not total).
    ///
    /// Used when `count_total = true` but no ORDER BY / GROUP BY / DISTINCT /
    /// aggregates — i.e. the order is natural (insertion order) so we can
    /// paginate on-the-fly while still counting everything.
    async fn read_counting(
        &self,
        query: &ReadQuery,
        interner: &Interner,
        filter_cb: Option<&dyn FilterCallback>,
        ctx: &FilterContext<'_>,
        batch_size: usize,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let (skip, take) = query.pagination.resolve();
        let skip = skip as usize;
        let limit = take.map(|t| t as usize);

        let proj = exec::SelectProjection::new(&query.select, interner);

        let stream = self.list_stream(batch_size);
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
    /// to determine `has_next` accurately, then stops. Memory ~ page_size.
    async fn read_streaming(
        &self,
        query: &ReadQuery,
        interner: &Interner,
        filter_cb: Option<&dyn FilterCallback>,
        ctx: &FilterContext<'_>,
        batch_size: usize,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let (skip, take) = query.pagination.resolve();
        let skip = skip as usize;
        let limit = take.map(|t| t as usize);

        let proj = exec::SelectProjection::new(&query.select, interner);

        let stream = self.list_stream(batch_size);
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
}
