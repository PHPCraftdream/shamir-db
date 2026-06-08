//! Read query execution on TableManager.
//!
//! Implements read(), index scan planning, and read execution strategies
//! (collecting, counting, streaming) for TableManager.

use std::time::Instant;

use futures::StreamExt;

use crate::query::filter::eval::{
    compile_filter, filter_value_to_inner, intern_field_path, FilterNode,
};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::{
    exec, At, OrderDirection, PaginationInfo, QueryResult, QueryStats, ReadQuery, SelectItem,
    Temporal,
};
use shamir_storage::error::DbResult;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::core::sort_codec;
use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::index::sorted_index_manager::decode_covering_projection;

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

    async fn try_plan_index2(
        &self,
        filter: &Filter,
        interner: &shamir_types::core::interner::Interner,
    ) -> Option<crate::index2::backend::IndexResult> {
        use crate::index2::backend::{FtsMode, IndexQuery};
        use crate::query::filter::eval::intern_field_path;

        if self.index2_registry().is_empty() {
            return None;
        }
        let registry = self.index2_registry();

        match filter {
            Filter::Fts { field, query, mode } => {
                let interned = intern_field_path(field, interner)?;
                let backend = registry.find_by_field_and_kind(&interned, "fts").await?;
                let tokens: Vec<u64> = backend.tokenize_query(query);
                let fts_mode = if mode == "or" {
                    FtsMode::OrAny
                } else {
                    FtsMode::AndAll
                };
                backend
                    .lookup(IndexQuery::Fts {
                        tokens,
                        mode: fts_mode,
                    })
                    .await
                    .ok()
            }
            Filter::VectorSimilarity { field, query, k } => {
                let interned = intern_field_path(field, interner)?;
                let backend = registry.find_by_field_and_kind(&interned, "vector").await?;
                backend
                    .lookup(IndexQuery::Vector {
                        vec: query.clone(),
                        k: *k,
                    })
                    .await
                    .ok()
            }
            Filter::Computed {
                field, cmp, value, ..
            } if cmp == "eq" => {
                let interned = intern_field_path(field, interner)?;
                let backend = registry
                    .find_by_field_and_kind(&interned, "functional")
                    .await?;
                let resolved = crate::query::filter::eval::filter_value_to_inner(value)?;
                let hash =
                    crate::index2::functional_backend::FunctionalBackend::hash_value(&resolved);
                backend
                    .lookup(IndexQuery::Point {
                        keys: smallvec::smallvec![hash.to_vec()],
                    })
                    .await
                    .ok()
            }
            _ => None,
        }
    }

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
            Filter::And { filters } => self.try_plan_and_index_scan(filters, interner),

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
                    if let Some(item) = items
                        .iter()
                        .find(|it| it.field_path == idx_path.path && it.lookup_sets.len() == 1)
                    {
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
            _ => Some(Filter::And { filters: remaining }),
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
    pub async fn read(&self, query: &ReadQuery, ctx: &FilterContext<'_>) -> DbResult<QueryResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // T4: temporal reads. `History` returns the per-key version
        // timeline for the records that currently match `where`;
        // `AsOf` (point-in-time via versioned indexes) is a later
        // slice — reject it with a clear error rather than silently
        // falling back to `Latest` (which would return wrong results).
        // `Latest` (the default) is unchanged and takes the normal
        // current-state pipeline below.
        match &query.temporal {
            Temporal::Latest => {}
            Temporal::AsOf { at } => {
                return self
                    .read_as_of(query, ctx, interner, at.clone(), start)
                    .await;
            }
            Temporal::History { .. } => {
                return self.read_history(query, ctx, interner, start).await;
            }
        }

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
                            let val =
                                crate::query::filter::eval::resolve_field(&record, &field_path);
                            let json_val = match val {
                                Some(v) => shamir_types::codecs::interned::inner_to_json_value(
                                    &v, interner,
                                )?,
                                None => serde_json::Value::Null,
                            };
                            let key = alias
                                .as_deref()
                                .unwrap_or_else(|| path.last().map(|s| s.as_str()).unwrap_or("min"))
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
                                value: None,
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
                    value: None,
                });
            }
        }

        // ── index2: FTS / Functional / Vector accelerated path ─────
        if let Some(ref filter) = query.r#where {
            if let Some(result) = self.try_plan_index2(filter, interner).await {
                let (rids_vec, index_tag) = match result {
                    crate::index2::backend::IndexResult::Set(rids) => {
                        (rids.into_iter().collect::<Vec<_>>(), "index2")
                    }
                    crate::index2::backend::IndexResult::Ranked(ranked) => (
                        ranked.into_iter().map(|(r, _)| r).collect::<Vec<_>>(),
                        "index2_ranked",
                    ),
                };
                if !rids_vec.is_empty() {
                    let inner_records = self.table().get_many(&rids_vec).await?;
                    let mut records = Vec::with_capacity(inner_records.len());
                    for inner in inner_records.into_iter().flatten() {
                        if let Ok(json) =
                            shamir_types::codecs::interned::inner_to_json_value(&inner, interner)
                        {
                            records.push(json)
                        }
                    }
                    let scanned = rids_vec.len() as u64;
                    let returned = records.len() as u64;
                    return Ok(crate::query::read::QueryResult {
                        records,
                        stats: Some(crate::query::read::QueryStats {
                            index_used: Some(index_tag.into()),
                            records_scanned: scanned,
                            records_returned: returned,
                            execution_time_us: start.elapsed().as_micros() as u64,
                        }),
                        pagination: None,
                        value: None,
                    });
                }
            }
        }

        // Try index scan first (legacy btree)
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
                            value: None,
                        });
                    }
                }

                return self
                    .read_index_scan(
                        query,
                        ctx,
                        interner,
                        idx_name,
                        &lookup_sets,
                        residual.as_ref(),
                        start,
                    )
                    .await;
            }
        }

        // Q1b: SELECT max(field) mirror of MIN — sorted-index walk
        // to the LAST key under the prefix. Requires reverse-iter
        // on Store (now in the trait via
        // `iter_range_stream_reverse`).
        if query.r#where.is_none()
            && query.group_by.is_none()
            && query.order_by.is_none()
            && !query.select.distinct
            && !query.count_total
            && query.pagination.is_none()
            && query.select.items.len() == 1
        {
            if let SelectItem::Aggregate {
                func: shamir_query_types::read::AggFunc::Max,
                field: shamir_query_types::read::AggregateField::Field(path),
                alias,
                ..
            } = &query.select.items[0]
            {
                if let Some(field_path) = intern_field_path(path, interner) {
                    if let Some(def) = self.sorted_indexes().find_by_field(&field_path) {
                        if let Some(id) =
                            self.sorted_indexes().lookup_max(def.name_interned).await?
                        {
                            let record = self.get(id).await?;
                            let val =
                                crate::query::filter::eval::resolve_field(&record, &field_path);
                            let json_val = match val {
                                Some(v) => shamir_types::codecs::interned::inner_to_json_value(
                                    &v, interner,
                                )?,
                                None => serde_json::Value::Null,
                            };
                            let key = alias
                                .as_deref()
                                .unwrap_or_else(|| path.last().map(|s| s.as_str()).unwrap_or("max"))
                                .to_string();
                            let mut obj = serde_json::Map::new();
                            obj.insert(key, json_val);
                            return Ok(QueryResult {
                                records: vec![serde_json::Value::Object(obj)],
                                stats: Some(QueryStats {
                                    index_used: Some(format!(
                                        "sorted_idx_{}_max",
                                        def.name_interned
                                    )),
                                    records_scanned: 1,
                                    records_returned: 1,
                                    execution_time_us: start.elapsed().as_micros() as u64,
                                }),
                                pagination: None,
                                value: None,
                            });
                        }
                    }
                }
            }
        }

        // Opt #6 — sorted-index ORDER BY field ASC LIMIT K fast path.
        //
        // When the query is exactly `ORDER BY field ASC LIMIT K` (or
        // `LIMIT K OFFSET m`) with no WHERE / GROUP BY / DISTINCT /
        // count_total / aggregates and a sorted index covers `field`,
        // skip the "collect all matching rows + sort + truncate"
        // pipeline. The sorted index already stores record_ids in
        // value-ascending order — `lookup_first_k(K+m)` walks the
        // first K+m entries in O(log N + K + m) and we just project
        // them.
        //
        // O(N log N) → O(log N + K + m). At N=10K, K=10 it's a ~1000×
        // asymptotic improvement; even at K=1000 the win is large
        // because the sort step disappears.
        //
        // Falls through to the existing paths when the shape doesn't
        // match (DESC, multi-field order_by, residual filter, etc.).
        if let Some((idx_name, take, skip, direction)) =
            self.try_plan_order_limit_fast_path(query, interner)
        {
            return self
                .read_order_limit_fast(query, ctx, interner, idx_name, take, skip, direction, start)
                .await;
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

        let filter_cb: Option<FilterNode> =
            query.r#where.as_ref().map(|f| compile_filter(f, interner));

        let needs_full_collect = has_group_by || has_agg || has_order || has_distinct;

        if needs_full_collect {
            self.read_collecting(query, ctx, interner, filter_cb.as_ref(), batch_size, start)
                .await
        } else if query.count_total {
            self.read_counting(query, interner, filter_cb.as_ref(), ctx, batch_size, start)
                .await
        } else {
            self.read_streaming(query, interner, filter_cb.as_ref(), ctx, batch_size, start)
                .await
        }
    }

    /// T4-asof: point-in-time read — return table state as it existed at
    /// the given `at` version or timestamp.
    ///
    /// Strategy: full-scan-at-version. Secondary/sorted indexes reflect the
    /// CURRENT state and cannot be used here — a record that matches a WHERE
    /// condition NOW may not have matched at `at`, and vice versa. We
    /// enumerate every record id, read each record AT the as-of version via
    /// `MvccStore::get_at`, apply the WHERE filter to the as-of value, then
    /// project. O(n) — versioned indexes are a later performance slice.
    ///
    /// Requires an MVCC-backed table; a non-MVCC table returns a clear error.
    ///
    /// `At::Version(v)` is used directly.
    /// `At::Timestamp(t)` is resolved via `MvccStore::version_at_or_before_ts`;
    /// if no version has a recorded ts ≤ `t` this returns a clear error rather
    /// than silently treating it as Latest.
    ///
    /// `Latest` and `History` arms are NOT handled here — `read()` routes them
    /// before reaching this method.
    async fn read_as_of(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        at: At,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let mvcc = self.mvcc_store_ref().ok_or_else(|| {
            shamir_storage::error::DbError::Validation(
                "AsOf temporal read requires an MVCC-backed table".to_string(),
            )
        })?;

        // ── 1. Resolve `at` to a concrete version number. ──────────────────
        let version: u64 = match at {
            At::Version(v) => v,
            At::Timestamp(t) => match mvcc.version_at_or_before_ts(t).await {
                Some(v) => v,
                None => {
                    return Err(shamir_storage::error::DbError::Validation(format!(
                            "AsOf(Timestamp({t})): no committed version with recorded ts ≤ {t}ms found; \
                             ensure the table has MVCC history and the timestamp is not earlier than all \
                             recorded versions"
                        )));
                }
            },
        };

        // ── 2. Compile the WHERE filter (will be applied to AS-OF values). ──
        let filter_cb: Option<FilterNode> =
            query.r#where.as_ref().map(|f| compile_filter(f, interner));

        // ── 3. Enumerate every record id via the same full-scan streaming that
        //       the normal no-index read path uses. For each id, read the AS-OF
        //       value from the MVCC history (`get_at`). Records that did not yet
        //       exist at `version` return `None` and are excluded.
        //
        //       NOTE: secondary/sorted indexes reflect the CURRENT state and are
        //       intentionally NOT used here. A versioned index is a later slice.
        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);

        let mut matched: Vec<(RecordId, InnerValue)> = Vec::new();
        let mut records_scanned: u64 = 0;

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            records_scanned += batch.len() as u64;
            for (id, _current_value) in batch {
                // Read the AS-OF value — this is NOT the current value; it is
                // the value the record had at `version` (or None if it did not
                // exist yet / was already deleted at that point).
                let asof_bytes = mvcc.get_at(&id.to_bytes(), version).await?;
                let Some(bytes) = asof_bytes else {
                    // Record did not exist at this version — exclude it.
                    continue;
                };
                let inner = match InnerValue::from_bytes(&bytes) {
                    Ok(v) => v,
                    Err(_) => continue, // corrupt entry — skip defensively
                };
                // Apply the WHERE filter to the AS-OF value (NOT the current
                // value). This ensures `AsOf` semantics: the filter evaluates
                // the world as it was at `version`.
                let passes = match filter_cb.as_ref() {
                    Some(cb) => cb.matches(&inner, ctx),
                    None => true,
                };
                if passes {
                    matched.push((id, inner));
                }
            }
        }

        // ── 4. Pipeline tail — same helpers as the collecting / index-scan
        //       paths. Apply projection, aggregates, order, pagination.
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);

        if let Some((paged, pagination)) = try_project_page_only(query, &matched, interner) {
            let records_returned = paged.len() as u64;
            return Ok(QueryResult {
                records: paged,
                stats: Some(QueryStats {
                    index_used: Some("temporal_asof".to_string()),
                    records_scanned,
                    records_returned,
                    execution_time_us: start.elapsed().as_micros() as u64,
                }),
                pagination,
                value: None,
            });
        }

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

        let records_returned = records.len() as u64;
        Ok(QueryResult {
            records,
            stats: Some(QueryStats {
                index_used: Some("temporal_asof".to_string()),
                records_scanned,
                records_returned,
                execution_time_us: start.elapsed().as_micros() as u64,
            }),
            pagination,
            value: None,
        })
    }

    /// T4-history: the per-record version timeline.
    ///
    /// Semantics: "the history of the records that currently match".
    /// The matched record ids are resolved with the EXISTING
    /// current-state filter path (a streaming scan + the compiled
    /// `where` callback — the same machinery `read_collecting` uses,
    /// but we keep only the ids). For each matched id we ask the
    /// table's [`MvccStore`] for its full version timeline
    /// ([`MvccStore::history_of`]), then apply the `History`
    /// range / order / limit.
    ///
    /// Each output row is the projected fields (via `apply_select` on
    /// the single `(id, decoded_version_value)` pair) PLUS `_version`
    /// (u64) and `_ts` (millis or null).
    ///
    /// Requires an MvccStore; a non-MVCC table returns a clear error.
    /// No versioned indexes are consulted (History is orthogonal to
    /// them). `Latest`/`AsOf` are NOT handled here — `read()` routes
    /// them before reaching this method.
    #[allow(clippy::too_many_lines)] // one cohesive read strategy
    async fn read_history(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let mvcc = self.mvcc_store_ref().ok_or_else(|| {
            shamir_storage::error::DbError::Validation(
                "History temporal read requires an MVCC-backed table".to_string(),
            )
        })?;

        // ── 1. Resolve matched record ids (reuse the current-state
        //     filter path). We stream every record, apply the compiled
        //     `where` callback, and keep only the RecordIds of the
        //     records that match RIGHT NOW. The current InnerValue is
        //     kept too — `history_of` already returns the current
        //     value, but having it lets us skip ids whose main value
        //     vanished between the match and the history call (a
        //     deleted record contributes only historical rows, which
        //     is the documented semantics; we still ask history_of
        //     for them).
        let filter_cb: Option<FilterNode> =
            query.r#where.as_ref().map(|f| compile_filter(f, interner));

        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);
        let mut matched_ids: Vec<RecordId> = Vec::new();
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (id, record) in batch {
                let passes = match filter_cb.as_ref() {
                    Some(cb) => cb.matches(&record, ctx),
                    None => true,
                };
                if passes {
                    matched_ids.push(id);
                }
            }
        }

        // ── 2. For each matched id, pull its full timeline.
        let temporal = &query.temporal;
        let (from, to, limit, order) = match temporal {
            Temporal::History {
                from,
                to,
                limit,
                order,
            } => (from.as_ref(), to.as_ref(), *limit, *order),
            // Unreachable: read() only routes History here.
            _ => unreachable!("read_history invoked for non-History temporal"),
        };

        // Collect every (id, version, ts, value) tuple across all
        // matched records, applying the from/to range filter inline.
        // `from`/`to` resolve `At::Version` directly against the
        // entry's version; `At::Timestamp(t)` filters by `ts_millis`
        // (entries with unknown ts are EXCLUDED from a ts-bounded
        // range — they can't be proven to lie inside it).
        #[inline]
        fn in_range(version: u64, ts: Option<u64>, from: Option<&At>, to: Option<&At>) -> bool {
            let ok_from = match from {
                None => true,
                Some(At::Version(v)) => version >= *v,
                Some(At::Timestamp(t)) => match ts {
                    Some(ts_val) => ts_val >= *t,
                    None => false,
                },
            };
            let ok_to = match to {
                None => true,
                Some(At::Version(v)) => version <= *v,
                Some(At::Timestamp(t)) => match ts {
                    Some(ts_val) => ts_val <= *t,
                    None => false,
                },
            };
            ok_from && ok_to
        }

        // Row shape: (record_id, version, ts, value_bytes).
        let mut rows: Vec<(RecordId, u64, Option<u64>, bytes::Bytes)> = Vec::new();
        for id in &matched_ids {
            let timeline = mvcc.history_of(&id.to_bytes()).await?;
            for entry in timeline {
                if in_range(entry.version, entry.ts_millis, from, to) {
                    rows.push((*id, entry.version, entry.ts_millis, entry.value));
                }
            }
        }

        // ── 3. Order by version (Asc default; Desc reverses).
        match order {
            OrderDirection::Asc => rows.sort_by_key(|(_, v, _, _)| *v),
            OrderDirection::Desc => rows.sort_by(|a, b| b.1.cmp(&a.1)),
        }

        // ── 4. Limit is applied over the WHOLE flattened result
        //     (documented): `limit` rows total, not per record.
        if let Some(n) = limit {
            rows.truncate(n as usize);
        }

        // ── 5. Decode each version's value bytes into an InnerValue
        //     and project via `apply_select` on the single
        //     (id, value) pair; then attach `_version` and `_ts`.
        let mut out_records: Vec<serde_json::Value> = Vec::with_capacity(rows.len());
        for (id, version, ts, value_bytes) in rows {
            // Decode the archived/current bytes into an InnerValue.
            // A corrupt entry is skipped (defensive — history bytes
            // are written by the engine itself, so this should never
            // fire in practice).
            let inner = match InnerValue::from_bytes(&value_bytes) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mut projected = exec::apply_select(&[(id, inner)], &query.select, interner);
            // apply_select returns one JSON value per input record.
            let mut row = projected
                .pop()
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            // Attach the timeline metadata. If projection produced a
            // non-object (e.g. a bare scalar SELECT), wrap it so the
            // metadata has a home.
            if let serde_json::Value::Object(map) = &mut row {
                map.insert(
                    "_version".to_string(),
                    serde_json::Value::Number(version.into()),
                );
                map.insert(
                    "_ts".to_string(),
                    match ts {
                        Some(t) => serde_json::Value::Number(t.into()),
                        None => serde_json::Value::Null,
                    },
                );
            } else {
                let mut map = serde_json::Map::new();
                map.insert("value".to_string(), row);
                map.insert(
                    "_version".to_string(),
                    serde_json::Value::Number(version.into()),
                );
                map.insert(
                    "_ts".to_string(),
                    match ts {
                        Some(t) => serde_json::Value::Number(t.into()),
                        None => serde_json::Value::Null,
                    },
                );
                row = serde_json::Value::Object(map);
            }
            out_records.push(row);
        }

        let records_returned = out_records.len() as u64;
        Ok(QueryResult {
            records: out_records,
            stats: Some(QueryStats {
                index_used: Some("temporal_history".to_string()),
                records_scanned: matched_ids.len() as u64,
                records_returned,
                execution_time_us: start.elapsed().as_micros() as u64,
            }),
            pagination: None,
            value: None,
        })
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
    #[allow(clippy::type_complexity)] // scan plan tuple; kept unpacked for caller convenience
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
    #[allow(clippy::too_many_arguments)] // read-path parameters mirror query plan fields
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
        // ── Covering index-only eligibility gate (slice A3) ──────────────────
        //
        // If ALL conditions below hold, we can serve the result entirely from
        // the index postings without fetching records from the data store.
        // Any failed guard falls through to the existing full-fetch path BELOW
        // (unchanged).
        //
        // Guard 1: an MvccStore is attached (version authority for freshness).
        // Guard 2: no residual filter (would need the full record to evaluate).
        // Guard 3: no GROUP BY, no ORDER BY, no count_total.
        // Guard 4: no aggregates, no DISTINCT.
        // Guard 5: SELECT items are non-empty and every item is a top-level Field.
        // Guard 6: the index definition is a covering index.
        // Guard 7: every selected field is present in the index's included_fields.
        if let Some(mvcc) = self.mvcc_store_ref() {
            if residual.is_none()
                && query.group_by.is_none()
                && query.order_by.is_none()
                && !query.count_total
                && !exec::has_aggregates(&query.select)
                && !query.select.distinct
                && !query.select.items.is_empty()
            {
                // Guard 5: every item must be SelectItem::Field { path: [single_seg], .. }
                let all_top_level_fields =
                    query.select.items.iter().all(
                        |item| matches!(item, SelectItem::Field { path, .. } if path.len() == 1),
                    );

                if all_top_level_fields {
                    // Collect the selected field names (single segment each).
                    let selected_fields: Vec<&str> = query
                        .select
                        .items
                        .iter()
                        .filter_map(|item| {
                            if let SelectItem::Field { path, .. } = item {
                                path.first().map(|s| s.as_str())
                            } else {
                                None
                            }
                        })
                        .collect();

                    // Guard 6: the definition is a covering index.
                    if let Some(def) = self
                        .sorted_indexes()
                        .find_by_name_interned(index_name)
                        .filter(|d| d.is_covering())
                    {
                        // Guard 7: every selected field is present in included_fields.
                        let all_covered = selected_fields.iter().all(|sel_field| {
                            def.included_fields
                                .iter()
                                .any(|inc| inc.len() == 1 && inc[0].as_str() == *sel_field)
                        });

                        if all_covered {
                            // ── Index-only execution path ─────────────────────
                            let entries = self
                                .sorted_indexes()
                                .lookup_range_with_values(index_name, lower_encoded, upper_encoded)
                                .await?;

                            let mut matched: Vec<(RecordId, InnerValue)> =
                                Vec::with_capacity(entries.len());
                            let mut fallback: Vec<RecordId> = Vec::new();

                            for (id, pv) in &entries {
                                match decode_covering_projection(pv) {
                                    Some((v, proj))
                                        if mvcc
                                            .live_version(&id.to_bytes())
                                            .is_none_or(|hwm| hwm == v) =>
                                    {
                                        // Posting is fresh — reconstruct the row.
                                        let mut inner_map = new_map();
                                        for (dotted_name, leaf) in proj {
                                            // dotted_name is a single segment here
                                            // (eligibility gate ensures top-level fields only).
                                            let key = interner
                                                .touch_ind(dotted_name.as_str())
                                                .map_err(|e| {
                                                    shamir_storage::error::DbError::Codec(
                                                        e.to_string(),
                                                    )
                                                })?
                                                .key()
                                                .clone();
                                            inner_map.insert(key, leaf);
                                        }
                                        matched.push((*id, InnerValue::Map(inner_map)));
                                    }
                                    _ => {
                                        // None (corrupt/empty posting) or version mismatch —
                                        // fall back to a full fetch.
                                        fallback.push(*id);
                                    }
                                }
                            }

                            // Resolve fallbacks with a single get_many call.
                            // A `None` result means the record was deleted after the
                            // posting was written — silently skip it (no phantom).
                            if !fallback.is_empty() {
                                let recs = self.table().get_many(&fallback).await?;
                                for (id, opt) in fallback.iter().zip(recs) {
                                    if let Some(record) = opt {
                                        matched.push((*id, record));
                                    }
                                    // None → deleted record; skip to prevent phantom reads.
                                }
                            }

                            let records_scanned = matched.len() as u64;

                            // Pipeline tail: no residual, no group_by, no aggregates,
                            // no order_by (all excluded by the eligibility guard above).
                            if let Some((paged, pagination)) =
                                try_project_page_only(query, &matched, interner)
                            {
                                let records_returned = paged.len() as u64;
                                return Ok(QueryResult {
                                    records: paged,
                                    stats: Some(QueryStats {
                                        index_used: Some(format!(
                                            "sorted_idx_{index_name}_covering"
                                        )),
                                        records_scanned,
                                        records_returned,
                                        execution_time_us: start.elapsed().as_micros() as u64,
                                    }),
                                    pagination,
                                    value: None,
                                });
                            }

                            let result = exec::apply_select(&matched, &query.select, interner);
                            let (paged, pagination) = exec::apply_pagination(
                                result,
                                &query.pagination,
                                query.count_total,
                            );
                            let records_returned = paged.len() as u64;
                            return Ok(QueryResult {
                                records: paged,
                                stats: Some(QueryStats {
                                    index_used: Some(format!("sorted_idx_{index_name}_covering")),
                                    records_scanned,
                                    records_returned,
                                    execution_time_us: start.elapsed().as_micros() as u64,
                                }),
                                pagination,
                                value: None,
                            });
                        }
                    }
                }
            }
        }
        // ── End covering index-only gate ─────────────────────────────────────
        // Fall through to the existing full-fetch path (byte-identical below).

        // 1. Lookup matching RecordIds from the sorted index.
        let record_ids = self
            .sorted_indexes()
            .lookup_range(index_name, lower_encoded, upper_encoded)
            .await?;

        // 2. Compile residual filter if present.
        let residual_cb: Option<FilterNode> = residual.map(|f| compile_filter(f, interner));

        // 3. Vectored fetch + per-record residual filter. One round
        //    trip to the data store via `Store::get_many`; stale
        //    index entries materialise as `None` and are silently
        //    skipped (same semantic as the previous NotFound branch).
        let id_vec: Vec<RecordId> = record_ids.iter().copied().collect();
        let records = self.table().get_many(&id_vec).await?;
        let mut matched: Vec<(RecordId, InnerValue)> = Vec::with_capacity(id_vec.len());
        for (id, opt) in id_vec.iter().zip(records) {
            if let Some(record) = opt {
                let passes = match &residual_cb {
                    Some(cb) => cb.matches(&record, ctx),
                    None => true,
                };
                if passes {
                    matched.push((*id, record));
                }
            }
        }

        // 4. Re-use the same pipeline tail as the equality index path
        //    by calling the same projection / sort / paginate helpers.
        //    Inline the bits we need from `read_index_scan` body.
        let records_scanned = matched.len() as u64;

        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);

        // Opt #3a (LIMIT push-down): plain filtered SELECT with LIMIT
        // and no in-memory ORDER BY / GROUP BY / DISTINCT / aggregates
        // projects only the page rows instead of every match.
        if let Some((paged, pagination)) = try_project_page_only(query, &matched, interner) {
            let records_returned = paged.len() as u64;
            return Ok(QueryResult {
                records: paged,
                stats: Some(QueryStats {
                    index_used: Some(format!("sorted_idx_{index_name}")),
                    records_scanned,
                    records_returned,
                    execution_time_us: start.elapsed().as_micros() as u64,
                }),
                pagination,
                value: None,
            });
        }

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
            value: None,
        })
    }

    /// Eligibility check for the ORDER BY + LIMIT K fast path
    /// (both ASC and DESC).
    ///
    /// Returns `Some((sorted_index_name, take, skip, direction))`
    /// when the query is:
    ///
    /// - `order_by: { items: [single_item] }` (either direction)
    /// - `pagination: LimitOffset` with a finite `limit` (paged form
    ///   normalised through `pagination.resolve()`)
    /// - no `where`, `group_by`, `select.distinct`, `count_total`,
    ///   no aggregate items in `select`
    /// - a sorted index covers the order_by field
    ///
    /// The fast path materialises only `skip + take` index entries
    /// in the requested order (asc via `lookup_first_k`, desc via
    /// `lookup_last_k`), then projects.
    pub fn try_plan_order_limit_fast_path(
        &self,
        query: &ReadQuery,
        interner: &Interner,
    ) -> Option<(u64, usize, usize, shamir_query_types::read::OrderDirection)> {
        // Shape guards.
        if query.r#where.is_some()
            || query.group_by.is_some()
            || query.select.distinct
            || query.count_total
            || exec::has_aggregates(&query.select)
        {
            return None;
        }
        let order_by = query.order_by.as_ref()?;
        if order_by.items.len() != 1 {
            return None;
        }
        let item = &order_by.items[0];
        // Pagination must yield a finite take.
        let (skip, take_opt) = query.pagination.resolve();
        let take = take_opt? as usize;
        if take == 0 {
            return None;
        }
        let skip = skip as usize;

        // Sorted index must cover the order_by field.
        let mgr = self.sorted_indexes();
        if !mgr.has_indexes() {
            return None;
        }
        let field_path = intern_field_path(&item.field, interner)?;
        let def = mgr.find_by_field(&field_path)?;

        Some((def.name_interned, take, skip, item.direction))
    }

    /// Execute the ORDER BY LIMIT K fast path: pull `skip + take`
    /// record ids from the sorted index in the requested direction,
    /// skip the offset, load + project.
    #[allow(clippy::too_many_arguments)] // read-path parameters mirror query plan fields
    async fn read_order_limit_fast(
        &self,
        query: &ReadQuery,
        _ctx: &FilterContext<'_>,
        interner: &Interner,
        index_name: u64,
        take: usize,
        skip: usize,
        direction: shamir_query_types::read::OrderDirection,
        start: Instant,
    ) -> DbResult<QueryResult> {
        use shamir_query_types::read::OrderDirection;

        let want = skip.checked_add(take).ok_or_else(|| {
            shamir_storage::error::DbError::Validation("LIMIT + OFFSET overflow".to_string())
        })?;

        let (ids, label) = match direction {
            OrderDirection::Asc => (
                self.sorted_indexes()
                    .lookup_first_k(index_name, want)
                    .await?,
                format!("sorted_idx_{index_name}_first_k"),
            ),
            OrderDirection::Desc => (
                self.sorted_indexes()
                    .lookup_last_k(index_name, want)
                    .await?,
                format!("sorted_idx_{index_name}_last_k"),
            ),
        };

        let records_scanned = ids.len() as u64;
        // Vectored fetch of the ids we actually need (post-skip,
        // pre-take). Stale ids → None; we collect Some until we hit
        // `take`. One trip to the data store, regardless of K.
        let needed: Vec<RecordId> = ids.into_iter().skip(skip).collect();
        let fetched = self.table().get_many(&needed).await?;
        let mut matched: Vec<(RecordId, InnerValue)> = Vec::with_capacity(take);
        for (id, opt) in needed.iter().zip(fetched) {
            if matched.len() == take {
                break;
            }
            if let Some(record) = opt {
                matched.push((*id, record));
            }
        }

        let result = exec::apply_select(&matched, &query.select, interner);
        let records_returned = result.len() as u64;

        Ok(QueryResult {
            records: result,
            stats: Some(QueryStats {
                index_used: Some(label),
                records_scanned,
                records_returned,
                execution_time_us: start.elapsed().as_micros() as u64,
            }),
            pagination: None,
            value: None,
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
    #[allow(clippy::too_many_arguments)] // read-path parameters mirror query plan fields
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
            let ids = self
                .index_manager_ref()
                .lookup_by_index(index_name, values)
                .await?;
            record_ids.extend(ids);
        }

        // 2. Compile residual filter if present
        let residual_cb: Option<FilterNode> = residual.map(|f| compile_filter(f, interner));

        // 3. Vectored fetch + per-record residual filter. Stale
        //    index entries materialise as None and are skipped.
        let id_vec: Vec<RecordId> = record_ids.iter().copied().collect();
        let records = self.table().get_many(&id_vec).await?;
        let mut matched: Vec<(RecordId, InnerValue)> = Vec::with_capacity(id_vec.len());
        for (id, opt) in id_vec.iter().zip(records) {
            if let Some(record) = opt {
                let passes = match &residual_cb {
                    Some(cb) => cb.matches(&record, ctx),
                    None => true,
                };
                if passes {
                    matched.push((*id, record));
                }
            }
        }

        let records_scanned = matched.len() as u64;

        // 4. Apply the rest of the pipeline (same as collecting path)
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);

        // Resolve index name for stats — needed by both paths below.
        let index_name_str = interner
            .get_str(&InternerKey::new(index_name))
            .map(|k| k.as_str().to_string())
            .unwrap_or_else(|| index_name.to_string());

        // Opt #3a (LIMIT push-down): plain filtered SELECT with LIMIT
        // and no in-memory ORDER BY / GROUP BY / DISTINCT / aggregates
        // projects only the page rows instead of every match.
        if let Some((paged, pagination)) = try_project_page_only(query, &matched, interner) {
            let elapsed = start.elapsed();
            let records_returned = paged.len() as u64;
            return Ok(QueryResult {
                records: paged,
                stats: Some(QueryStats {
                    index_used: Some(index_name_str),
                    records_scanned,
                    records_returned,
                    execution_time_us: elapsed.as_micros() as u64,
                }),
                pagination,
                value: None,
            });
        }

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

        Ok(QueryResult {
            records,
            stats: Some(QueryStats {
                index_used: Some(index_name_str),
                records_scanned,
                records_returned,
                execution_time_us: elapsed.as_micros() as u64,
            }),
            pagination,
            value: None,
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
        filter_cb: Option<&FilterNode>,
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
            value: None,
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
        filter_cb: Option<&FilterNode>,
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
            value: None,
        })
    }

    /// Streaming path: SELECT + PAGINATION only (no ORDER BY, GROUP BY, DISTINCT,
    /// aggregates, count_total). Projects on-the-fly, fetches up to `limit + 1`
    /// to determine `has_next` accurately, then stops. Memory ~ page_size.
    async fn read_streaming(
        &self,
        query: &ReadQuery,
        interner: &Interner,
        filter_cb: Option<&FilterNode>,
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
            Some(PaginationInfo::compute(&query.pagination, None).with_has_next(has_next))
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
            value: None,
        })
    }
}

/// Opt #3a — LIMIT push-down for index scan paths.
///
/// When the query has **no in-memory ORDER BY, no GROUP BY, no DISTINCT,
/// no aggregates** (plain filtered SELECT with LIMIT [+ optional OFFSET]),
/// project ONLY the page rows instead of every matched row. Semantics are
/// byte-identical to the full-projection path: LIMIT without ORDER BY
/// already returns "the first N in scan order"; we just stop projecting
/// the rest.
///
/// Returns `None` (caller falls through to the full pipeline) when any
/// of the gate conditions fails or when there is no finite LIMIT to push.
/// `count_total` is preserved: the full match count equals `matched.len()`
/// — we can read it without projecting any row beyond the page.
fn try_project_page_only(
    query: &ReadQuery,
    matched: &[(RecordId, InnerValue)],
    interner: &Interner,
) -> Option<(Vec<serde_json::Value>, Option<PaginationInfo>)> {
    // Gate: every condition below disables the push-down.
    if query.order_by.is_some()
        || query.group_by.is_some()
        || query.select.distinct
        || exec::has_aggregates(&query.select)
    {
        return None;
    }

    // Need pagination or count_total — otherwise the original path is
    // already O(matches) and there's nothing to push down.
    if query.pagination.is_none() && !query.count_total {
        return None;
    }

    let (skip_u64, take_u64) = query.pagination.resolve();
    // Require a finite limit to project: without it, the page is the
    // whole tail and we'd still project every row. Skip-only optimisation
    // (no limit, offset > 0) would only save the prefix — not worth it
    // here; let the fall-through path handle it.
    let take = take_u64? as usize;
    let skip = skip_u64 as usize;

    let total_matches = matched.len();
    let total_u64 = total_matches as u64;

    // Slice the page from `matched` before projection.
    let page_start = skip.min(total_matches);
    let page_end = skip.saturating_add(take).min(total_matches);
    let page_slice = &matched[page_start..page_end];

    let proj = exec::SelectProjection::new(&query.select, interner);
    let mut paged: Vec<serde_json::Value> = Vec::with_capacity(page_slice.len());
    for (_, record) in page_slice {
        paged.push(proj.project(record, interner));
    }

    // Pagination metadata mirrors `apply_pagination`'s semantics: when
    // count_total is set, `total_count` is the full match count
    // (= matched.len()); otherwise we still get page_size / has_prev
    // from PaginationInfo::compute.
    let pagination = if query.pagination.is_none() && query.count_total {
        // count_total without pagination — same shape as apply_pagination.
        Some(PaginationInfo {
            total_count: Some(total_u64),
            total_pages: None,
            current_page: None,
            page_size: None,
            has_next: false,
            has_prev: false,
        })
    } else {
        let total_for_info = if query.count_total {
            Some(total_u64)
        } else {
            None
        };
        Some(PaginationInfo::compute(&query.pagination, total_for_info))
    };

    Some((paged, pagination))
}
