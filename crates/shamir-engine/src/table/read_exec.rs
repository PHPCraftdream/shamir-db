//! Read query execution on TableManager.
//!
//! Implements read(), index scan planning, and read execution strategies
//! (collecting, counting, streaming) for TableManager.

use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;

use crate::query::filter::eval::{compile_filter, intern_field_path, FilterNode};
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{
    exec, ExplainPlan, GroupBy, PaginationInfo, PlanType, QueryRecord, QueryResult, QueryStats,
    ReadQuery, Select, SelectItem, Temporal,
};
use bytes::Bytes;
use serde_bytes::ByteBuf;
use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_query_types::batch::ResultEncoding;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::codecs::interned::record_view_to_id_msgpack;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::RecordRef;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use super::record_cow::RecordCow;
use super::table_manager::TableManager;

/// Boxed, `Send`-able stream of decoded record batches used by the three scan
/// execution paths (`read_collecting`, `read_counting`, `read_streaming`).
type DynBatchStream<'a> = std::pin::Pin<
    Box<
        dyn futures::Stream<Item = shamir_storage::error::DbResult<Vec<(RecordId, RecordCow)>>>
            + Send
            + 'a,
    >,
>;

// ============================================================================
// S-read: simple-select gate
//
// A query is "simple" (eligible for the Id-keyed pass-through path) when:
//   - the select is `SELECT *` (contains at least one `SelectItem::All`, or
//     the items list is empty which defaults to all), AND no GROUP BY; OR
//   - the select is a plain field projection: every item is
//     `SelectItem::Field { alias: None }` — no aliases, no aggregates, no
//     computed/Function/Expression items.
//   - AND group_by is None.
//
// Falls back (returns false) for GROUP BY, aggregates, aliases, Function,
// Expression, CountAll — those need server-side de-interning or computation.
// ============================================================================

/// Returns `true` when the select + group_by combination is "simple" —
/// eligible for the id-keyed pass-through path.
///
/// Fallback triggers (returns `false`):
/// - Any `SelectItem` that is not `Field` or `All`: aggregates (`Aggregate`,
///   `AggregateFn`, `CountAll`), `Function`, `Expression`.
/// - Any `SelectItem::Field` that has an `alias` — the alias renames the
///   output key, which cannot be represented in an id-keyed result without
///   interning the alias.
/// - `group_by.is_some()`.
fn is_select_simple(select: &Select, group_by: Option<&GroupBy>) -> bool {
    if group_by.is_some() {
        return false;
    }
    for item in &select.items {
        match item {
            SelectItem::All => {}
            SelectItem::Field { alias, .. } => {
                if alias.is_some() {
                    return false;
                }
            }
            // Aggregates, computed, functions → fallback.
            SelectItem::Aggregate { .. }
            | SelectItem::AggregateFn { .. }
            | SelectItem::CountAll { .. }
            | SelectItem::Function { .. }
            | SelectItem::Expression { .. } => return false,
        }
    }
    true
}

/// Returns `true` when the select is `SELECT *` (all fields).
/// Empty items list is treated the same as `SelectItem::All`.
fn is_select_all(select: &Select) -> bool {
    select.items.is_empty() || select.items.iter().any(|i| matches!(i, SelectItem::All))
}

/// Intern the top-level field name (last path segment) of each plain
/// `SelectItem::Field` item and return the resulting interned-key list.
/// Called only when `is_select_simple` is true and `is_select_all` is false.
///
/// Returns `None` when any path is un-internable (miss in the interner) —
/// that field does not exist in this table so the projection would silently
/// drop it; we fall back to the Name path to preserve behaviour parity.
fn intern_simple_projection_ids(select: &Select, interner: &Interner) -> Option<Vec<InternerKey>> {
    let mut ids = Vec::with_capacity(select.items.len());
    for item in &select.items {
        if let SelectItem::Field { path, .. } = item {
            // For a simple field path `["a", "b"]` the stored id is the TOP-LEVEL
            // key only (`a`). Nested fields are reached through the map hierarchy
            // in the stored bytes — the projection copies the whole top-level value.
            // For a single-segment path `["a"]`, the id is `a` directly.
            // Multi-segment paths would require hierarchical projection which
            // `record_view_to_id_msgpack` does not support → fall back.
            if path.len() != 1 {
                return None;
            }
            let key = path.first()?;
            // `get_ind` is a read-only lookup (no new id allocation).
            let k = interner.get_ind(key)?;
            ids.push(k);
        }
    }
    Some(ids)
}

impl TableManager {
    // ============================================================================
    // Read query execution
    // ============================================================================

    /// Execute a read query pipeline (Name encoding — server de-interns rows).
    ///
    /// Tries index scan first if a suitable index exists for the WHERE clause.
    /// Falls back to streaming scan otherwise.
    ///
    /// Streaming scan has three sub-strategies:
    /// 1. **Streaming** — early termination, memory ~ page_size
    /// 2. **Counting** — count_total without ORDER BY, memory ~ page_size
    /// 3. **Collecting** — ORDER BY / GROUP BY / DISTINCT / aggregates
    pub async fn read(&self, query: &ReadQuery, ctx: &FilterContext<'_>) -> DbResult<QueryResult> {
        self.read_impl(query, ctx, None, ResultEncoding::Name).await
    }

    /// Like [`read`] but honours a client-requested [`ResultEncoding`].
    ///
    /// When `encoding == Id` and the query is "simple" (SELECT * or plain
    /// field projection, no GROUP BY / aggregates / aliases / computed), rows
    /// are returned as [`QueryRecord::IdBytes`] — raw id-keyed storage msgpack,
    /// no server de-interning.  For everything else falls back to
    /// [`ResultEncoding::Name`] (R5 de-intern path, fully intact).
    pub async fn read_with_encoding(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        encoding: ResultEncoding,
    ) -> DbResult<QueryResult> {
        self.read_impl(query, ctx, None, encoding).await
    }

    /// tx-aware variant of [`read`] used by [`read_tx`] to fuse the SSI
    /// read-set recording pass into the single scan that emits rows.
    /// For the full-scan fallback the three sub-methods use tx-aware streams
    /// so no second scan is needed. For index/shortcut paths, predicate-level
    /// SSI recording (already installed by `read_tx` before this call) is
    /// sufficient.
    pub(super) async fn read_for_tx(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<QueryResult> {
        self.read_impl(query, ctx, tx, ResultEncoding::Name).await
    }

    /// Like [`read_for_tx`] but honours a client-requested [`ResultEncoding`].
    pub(super) async fn read_for_tx_with_encoding(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
        encoding: ResultEncoding,
    ) -> DbResult<QueryResult> {
        self.read_impl(query, ctx, tx, encoding).await
    }

    async fn read_impl(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
        encoding: ResultEncoding,
    ) -> DbResult<QueryResult> {
        let start = Instant::now();
        let batch_size = shamir_tunables::store_defaults::FULL_SCAN_BATCH;
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

        // ── EXPLAIN dry-run: planner only, no materialisation ──────────
        if query.explain {
            let plan = self.build_explain_plan(query, interner);
            return Ok(QueryResult {
                records: Vec::new(),
                stats: None,
                pagination: None,
                value: None,
                explain: Some(plan),
                skipped: false,
            });
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
                            let qv_val = match val {
                                Some(v) => {
                                    shamir_types::codecs::interned::inner_value_to_query_value(
                                        &v, interner,
                                    )?
                                }
                                None => QueryValue::Null,
                            };
                            let key = alias
                                .as_deref()
                                .unwrap_or_else(|| path.last().map(|s| s.as_str()).unwrap_or("min"))
                                .to_string();
                            let mut obj = new_map_wc(1);
                            obj.insert(key, qv_val);
                            return Ok(QueryResult {
                                records: vec![crate::query::read::QueryRecord::Direct(
                                    QueryValue::Map(obj),
                                )],
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
                                explain: None,
                                skipped: false,
                            });
                        }
                    }
                }
            }

            if let SelectItem::CountAll { alias } = &query.select.items[0] {
                let count: u64 = self.counter().get().await?;
                let key = alias.as_deref().unwrap_or("count").to_string();
                let mut obj = new_map_wc(1);
                obj.insert(key, QueryValue::Int(count as i64));
                return Ok(QueryResult {
                    records: vec![crate::query::read::QueryRecord::Direct(QueryValue::Map(
                        obj,
                    ))],
                    stats: Some(QueryStats {
                        index_used: Some("__record_counter__".to_string()),
                        records_scanned: 0,
                        records_returned: 1,
                        execution_time_us: start.elapsed().as_micros() as u64,
                    }),
                    pagination: None,
                    value: None,
                    explain: None,
                    skipped: false,
                });
            }
        }

        // ── V3.1: filtered ANN — And([VectorSimilarity, ...residual]) ──
        // Recognise the filtered-vector pattern BEFORE the generic index2
        // path so we can run ANN-with-oversample + post-filter. Falls
        // through to the legacy paths when the shape doesn't match.
        if let Some(ref filter) = query.r#where {
            if let Some(fvq) = super::filtered_vector::try_extract_filtered_vector_query(filter) {
                return self
                    .read_filtered_vector_scan(query, ctx, interner, &fvq, tx, start)
                    .await;
            }
        }

        // ── index2: FTS / Functional / Vector accelerated path ─────
        //
        // CRIT-7: an `IndexResult::Set`/`Ranked` from index2 is the
        // AUTHORITATIVE, complete answer for this filter — including when
        // it is EMPTY. An empty set means "zero rows match", full stop.
        // We must NOT fall through to the legacy btree / full-scan paths
        // when `rids_vec` is empty:
        //   - FTS / functional: the full scan would re-derive the same
        //     empty answer at O(N) needless cost (tokenising every row
        //     again), plus mis-attribute `index_used`.
        //   - VectorSimilarity: the bare predicate compiles to
        //     `FilterNode::True` (it has no row-level representation), so
        //     a full-scan fall-through returns EVERY row in the table
        //     instead of zero — a correctness bug.
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
                // Preserve full match count for pagination metadata BEFORE
                // any page slice — clients rely on count_total for UI.
                let count_total = rids_vec.len() as u64;

                // Opt #5 (1000×-class): push pagination into index2 path.
                // index2 returns a pre-filtered, pre-ranked RID list with no
                // residual predicate, so it is safe to slice [skip..skip+take]
                // before calling get_many. Gate: finite LIMIT must be present.
                // Note: an empty `rids_vec` slices down to `&[]`, and
                // `get_many_bytes(&[])` short-circuits without a storage
                // round-trip — so the empty case is O(1).
                let (skip_u64, take_opt) = query.pagination.resolve();
                let rids_slice: &[RecordId] = if let Some(take_u64) = take_opt {
                    let skip = skip_u64 as usize;
                    let take = take_u64 as usize;
                    let lo = skip.min(rids_vec.len());
                    let hi = lo.saturating_add(take).min(rids_vec.len());
                    &rids_vec[lo..hi]
                } else {
                    &rids_vec
                };

                // S3: zero-copy bytes path — index2 is plain SELECT only.
                let raw_records = self.get_many_bytes(rids_slice).await?;
                let mut records = Vec::with_capacity(raw_records.len());
                for bytes in raw_records.into_iter().flatten() {
                    let qv = match shamir_types::record_view::RecordView::new(&bytes) {
                        Ok(view) => view.to_query_value(interner),
                        Err(_) => match InnerValue::from_bytes(bytes) {
                            Ok(iv) => {
                                match shamir_types::codecs::interned::inner_value_to_query_value(
                                    &iv, interner,
                                ) {
                                    Ok(q) => q,
                                    Err(_) => continue,
                                }
                            }
                            Err(_) => continue,
                        },
                    };
                    records.push(crate::query::read::QueryRecord::Direct(qv));
                }
                let returned = records.len() as u64;
                let pagination = if query.pagination.is_none() {
                    None
                } else {
                    Some(PaginationInfo::compute(
                        &query.pagination,
                        Some(count_total),
                    ))
                };
                return Ok(crate::query::read::QueryResult {
                    records,
                    stats: Some(crate::query::read::QueryStats {
                        index_used: Some(index_tag.into()),
                        records_scanned: count_total,
                        records_returned: returned,
                        execution_time_us: start.elapsed().as_micros() as u64,
                    }),
                    pagination,
                    value: None,
                    explain: None,
                    skipped: false,
                });
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
                        let mut obj = new_map_wc(1);
                        obj.insert(key, QueryValue::Int(total as i64));
                        return Ok(QueryResult {
                            records: vec![crate::query::read::QueryRecord::Direct(
                                QueryValue::Map(obj),
                            )],
                            stats: Some(QueryStats {
                                index_used: Some(format!("idx_{idx_name}")),
                                records_scanned: total,
                                records_returned: 1,
                                execution_time_us: start.elapsed().as_micros() as u64,
                            }),
                            pagination: None,
                            value: None,
                            explain: None,
                            skipped: false,
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
                            let qv_val = match val {
                                Some(v) => {
                                    shamir_types::codecs::interned::inner_value_to_query_value(
                                        &v, interner,
                                    )?
                                }
                                None => QueryValue::Null,
                            };
                            let key = alias
                                .as_deref()
                                .unwrap_or_else(|| path.last().map(|s| s.as_str()).unwrap_or("max"))
                                .to_string();
                            let mut obj = new_map_wc(1);
                            obj.insert(key, qv_val);
                            return Ok(QueryResult {
                                records: vec![crate::query::read::QueryRecord::Direct(
                                    QueryValue::Map(obj),
                                )],
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
                                explain: None,
                                skipped: false,
                            });
                        }
                    }
                }
            }
        }

        // Opt #6b — sorted-index keyset-seek (Pagination::After) fast path.
        //
        // When the query carries `Pagination::After { key: [v], limit }`
        // and matches the ORDER BY single-column shape, use the sorted
        // index to seek directly past the key. Mirrors #6 but with a
        // bounded range lookup instead of first_k / last_k.
        //
        // Checked BEFORE #6 because `Pagination::After` also resolves to
        // a finite (skip=0, take=limit) pair — without this ordering the
        // generic ORDER BY + LIMIT path would shadow the seek.
        if let Some((idx_name, encoded_key, after_id, limit, direction)) =
            self.try_plan_keyset_seek(query, interner)
        {
            return self
                .read_keyset_seek(
                    query,
                    ctx,
                    interner,
                    idx_name,
                    &encoded_key,
                    after_id.as_ref(),
                    limit,
                    direction,
                    start,
                )
                .await;
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

        // Range-from-AND extraction: try to pull a range predicate out
        // of an AND filter and use the sorted index for the range scan,
        // with remaining conjuncts as residual filter.
        if let Some(ref filter) = query.r#where {
            if let Some((idx_name, lo, hi, residual)) =
                self.try_plan_and_range_index_scan(filter, interner)
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

        let filter_cb: Option<Arc<FilterNode>> = query
            .r#where
            .as_ref()
            .map(|f| Arc::new(compile_filter(f, interner)));

        let needs_full_collect = has_group_by || has_agg || has_order || has_distinct;

        if needs_full_collect {
            // S-read fallback: ORDER BY / GROUP BY / DISTINCT / aggregates
            // require in-memory collection and QueryValue-based post-processing.
            // Id encoding is not applicable here — fall back to Name.
            self.read_collecting(
                query,
                ctx,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                batch_size,
                start,
                tx,
            )
            .await
        } else if query.count_total {
            self.read_counting(
                query,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                ctx,
                batch_size,
                start,
                tx,
            )
            .await
        } else {
            self.read_streaming(
                query,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                ctx,
                batch_size,
                start,
                tx,
                encoding,
            )
            .await
        }
    }

    /// Collecting path: streams batches, accumulates what's needed, then applies
    /// GROUP BY / aggregates / ORDER BY / DISTINCT / PAGINATION.
    ///
    /// For GROUP BY / aggregates — accumulates raw InnerValues (needed for
    /// field extraction). For plain SELECT + ORDER BY / DISTINCT — accumulates
    /// already-projected QueryValues (smaller footprint than raw records).
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn read_collecting(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        filter_cb: Option<&FilterNode>,
        pre_filter: Option<Arc<FilterNode>>,
        batch_size: usize,
        start: Instant,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<QueryResult> {
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);
        let needs_raw = has_group_by || has_agg;

        // AGG #54 — compute the referenced top-level field id set ONCE per
        // query. When provably complete + concrete (`Some`), the Borrowed arm
        // below decodes ONLY those subtrees instead of the full record tree;
        // when the gate falls back (`None`), it does the original full
        // `InnerValue::from_bytes` decode. The Owned arm is untouched (the
        // tree is already materialised — no bytes to prune).
        // S4: the aggregate path now feeds RecordView lenses over raw Bytes.
        // The AGG #54 prune_ids/prune_to_inner decode-prune is obsolete (the
        // lens already reads only referenced fields, skipping unreferenced
        // ones with O(1) skips, no tree decode).

        // Use bytes-level pre-filter when a compiled filter is present: rows
        // that definitely don't match are skipped before full InnerValue decode.
        // Both arms are boxed to unify the two opaque `impl Stream` types.
        // When a tx context is present, use tx-aware list_stream_tx so SSI
        // read-set recording is fused into this single scan. The bytes-level
        // pre-filter is skipped in that case — SSI correctness requires
        // recording every candidate row (not just pre-filter survivors), and
        // the compiled filter_cb in the loop below is the authoritative gate.
        let mut stream: DynBatchStream<'_> = if tx.is_some() {
            Box::pin(self.list_stream_tx(tx, batch_size))
        } else if let Some(pf) = pre_filter {
            Box::pin(self.list_stream_filtered(batch_size, pf))
        } else {
            Box::pin(self.list_stream(batch_size))
        };

        let mut records_scanned: u64 = 0;

        // Two accumulation modes: raw Bytes (for aggregates) or projected
        // QueryRecord (for plain SELECT). S4: the aggregate arm now carries
        // Bytes + per-row RecordView instead of a full InnerValue tree.
        let mut raw_acc: Vec<(RecordId, Bytes)> = Vec::new();
        let mut rec_acc: Vec<crate::query::read::QueryRecord> = Vec::new();
        let proj = if !needs_raw {
            Some(exec::SelectProjection::new(
                &query.select,
                interner,
                ctx.scalars.clone(),
            ))
        } else {
            None
        };

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            records_scanned += batch.len() as u64;
            for (id, cow) in batch {
                match cow {
                    RecordCow::Borrowed(b) => {
                        let view = match shamir_types::record_view::RecordView::new(&b) {
                            Ok(v) => v,
                            Err(_) => continue, // malformed row → skip
                        };
                        let passes = match filter_cb {
                            Some(cb) => cb.matches(&view, ctx),
                            None => true,
                        };
                        if passes {
                            if needs_raw {
                                // S4: push raw bytes; the aggregate pipeline
                                // builds a RecordView per row. No InnerValue
                                // decode here.
                                raw_acc.push((id, b));
                            } else {
                                rec_acc.push(crate::query::read::QueryRecord::Direct(
                                    proj.as_ref().unwrap().project_value(&view, interner),
                                ));
                            }
                        }
                    }
                    RecordCow::Owned(record) => {
                        let passes = match filter_cb {
                            Some(cb) => cb.matches(&record, ctx),
                            None => true,
                        };
                        if passes {
                            if needs_raw {
                                // S4: the Owned arm carries an already-decoded
                                // InnerValue (e.g. from MVCC). Re-encode to
                                // bytes once so the aggregate lens can consume
                                // it. This is the cold path (the hot Borrowed
                                // arm avoids the re-encode).
                                match record.to_bytes() {
                                    Ok(bytes) => raw_acc.push((id, bytes)),
                                    // Malformed tree — skip defensively.
                                    Err(_) => continue,
                                }
                            } else {
                                rec_acc.push(crate::query::read::QueryRecord::Direct(
                                    proj.as_ref().unwrap().project_value(&record, interner),
                                ));
                            }
                        }
                    }
                }
            }
        }

        let mut qv_result: Vec<shamir_types::types::value::QueryValue> = if has_group_by {
            let group_by = query.group_by.as_ref().unwrap();
            exec::apply_group_by(&raw_acc, group_by, &query.select, interner, ctx)
        } else if has_agg {
            exec::apply_aggregate_all(&raw_acc, &query.select, interner, ctx.scalars.clone())
        } else {
            rec_acc
                .into_iter()
                .map(|r| match r {
                    crate::query::read::QueryRecord::Direct(qv) => qv,
                    other => other.as_value().into_owned(),
                })
                .collect()
        };

        // Post-process directly on QueryValue — no legacy round-trip.
        // Both aggregate and non-aggregate paths now produce QueryValue
        // and go through the QV post-processors.
        if query.select.distinct {
            qv_result = exec::apply_distinct_qv(qv_result);
        }
        // Top-K path: when ORDER BY + finite LIMIT and no distinct/group_by,
        // use a bounded heap (O(K) memory) instead of full sort (O(N) memory).
        let (skip_resolved, take_resolved) = query.pagination.resolve();
        let use_topk = query.order_by.is_some()
            && take_resolved.is_some()
            && !query.select.distinct
            && !has_group_by
            && !has_agg;

        let (paged, pagination) = if use_topk {
            let order_by = query.order_by.as_ref().unwrap();
            let skip = skip_resolved as usize;
            let take = take_resolved.unwrap() as usize;
            let topk_result = exec::apply_order_by_topk(qv_result, order_by, skip, take);
            // count_total with top-K: we don't know the total from the
            // heap alone — but we tracked records_scanned. For true
            // count_total, the full-sort path is needed; top-K is memory-opt
            // only. Guard: count_total is already excluded above via the
            // `read_counting` path dispatch.
            //
            // #128 regression fix: the top-K LIMIT fast path emits the same
            // pagination metadata as every other LIMIT path, via the shared
            // `fast_path_pagination` helper (count_total == false → total
            // None). Returning a bare `None` here once silently dropped
            // pagination for every `ORDER BY` + `LIMIT` query.
            (topk_result, exec::fast_path_pagination(&query.pagination))
        } else {
            if let Some(ref order_by) = query.order_by {
                exec::apply_order_by_qv(&mut qv_result, order_by);
            }
            exec::apply_pagination(qv_result, &query.pagination, query.count_total)
        };
        let final_records: Vec<crate::query::read::QueryRecord> = paged
            .into_iter()
            .map(crate::query::read::QueryRecord::Direct)
            .collect();

        let elapsed = start.elapsed();
        let records_returned = final_records.len() as u64;
        let records = final_records;

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
            explain: None,
            skipped: false,
        })
    }

    /// Counting path: streams all records, counts total matched, but only
    /// keeps the requested page in memory. Memory ~ page_size (not total).
    ///
    /// Used when `count_total = true` but no ORDER BY / GROUP BY / DISTINCT /
    /// aggregates — i.e. the order is natural (insertion order) so we can
    /// paginate on-the-fly while still counting everything.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn read_counting(
        &self,
        query: &ReadQuery,
        interner: &Interner,
        filter_cb: Option<&FilterNode>,
        pre_filter: Option<Arc<FilterNode>>,
        ctx: &FilterContext<'_>,
        batch_size: usize,
        start: Instant,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<QueryResult> {
        let (skip, take) = query.pagination.resolve();
        let skip = skip as usize;
        let limit = take.map(|t| t as usize);

        let proj = exec::SelectProjection::new(&query.select, interner, ctx.scalars.clone());

        // When a tx context is present, use tx-aware list_stream_tx so SSI
        // read-set recording is fused into this single scan (see read_collecting).
        let mut stream: DynBatchStream<'_> = if tx.is_some() {
            Box::pin(self.list_stream_tx(tx, batch_size))
        } else if let Some(pf) = pre_filter {
            Box::pin(self.list_stream_filtered(batch_size, pf))
        } else {
            Box::pin(self.list_stream(batch_size))
        };

        let mut records_scanned: u64 = 0;
        let mut matched_total: u64 = 0;
        let mut result: Vec<crate::query::read::QueryRecord> = Vec::new();

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            records_scanned += batch.len() as u64;

            for (_, cow) in &batch {
                // Inline helper: filter + project via the correct RecordRef impl.
                // The RecordView is built once and reused for both matches+project.
                macro_rules! count_row {
                    ($rec:expr) => {{
                        let passes = match filter_cb {
                            Some(cb) => cb.matches($rec, ctx),
                            None => true,
                        };
                        if !passes {
                            continue;
                        }
                        let idx = matched_total as usize;
                        matched_total += 1;
                        if idx >= skip {
                            if let Some(lim) = limit {
                                if idx < skip + lim {
                                    result.push(crate::query::read::QueryRecord::Direct(
                                        proj.project_value($rec, interner),
                                    ));
                                }
                            } else {
                                result.push(crate::query::read::QueryRecord::Direct(
                                    proj.project_value($rec, interner),
                                ));
                            }
                        }
                    }};
                }

                match cow {
                    RecordCow::Borrowed(b) => {
                        let view = match shamir_types::record_view::RecordView::new(b) {
                            Ok(v) => v,
                            Err(_) => continue, // malformed row → skip
                        };
                        count_row!(&view);
                    }
                    RecordCow::Owned(record) => {
                        count_row!(record);
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
            explain: None,
            skipped: false,
        })
    }

    /// Streaming path: SELECT + PAGINATION only (no ORDER BY, GROUP BY, DISTINCT,
    /// aggregates, count_total). Projects on-the-fly, fetches up to `limit + 1`
    /// to determine `has_next` accurately, then stops. Memory ~ page_size.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn read_streaming(
        &self,
        query: &ReadQuery,
        interner: &Interner,
        filter_cb: Option<&FilterNode>,
        pre_filter: Option<Arc<FilterNode>>,
        ctx: &FilterContext<'_>,
        batch_size: usize,
        start: Instant,
        tx: Option<&shamir_tx::TxContext>,
        encoding: ResultEncoding,
    ) -> DbResult<QueryResult> {
        let (skip, take) = query.pagination.resolve();
        let skip = skip as usize;
        let limit = take.map(|t| t as usize);

        // S-read: determine whether to emit id-keyed pass-through rows.
        //
        // Conditions for the Id path:
        //   1. Client explicitly requested Id encoding.
        //   2. The query is "simple": no GROUP BY, no aggregates, no aliases,
        //      no computed/Function items.  ORDER BY / DISTINCT / count_total
        //      go through read_collecting/read_counting and are not eligible.
        //   3. Projection is either SELECT * (verbatim bytes) or plain field
        //      projection with single-segment paths (record_view_to_id_msgpack).
        //
        // Any condition failure → fall back to the Name path (R5 de-intern).
        // Fallback is correct + lossless; the Name path is fully intact.
        let use_id_encoding = matches!(encoding, ResultEncoding::Id)
            && is_select_simple(&query.select, query.group_by.as_ref());

        // Pre-compute whether the simple select is SELECT * or a projection.
        let id_is_all = use_id_encoding && is_select_all(&query.select);

        // For plain projection, intern the selected field ids once per query.
        // If interning fails (unknown field), fall back to the Name path.
        let id_projection_ids: Option<Vec<InternerKey>> = if use_id_encoding && !id_is_all {
            intern_simple_projection_ids(&query.select, interner)
        } else {
            None
        };

        // If projection interning failed, fall back to Name for the whole query.
        let use_id_encoding = use_id_encoding && (id_is_all || id_projection_ids.is_some());

        let proj = exec::SelectProjection::new(&query.select, interner, ctx.scalars.clone());

        // When a tx context is present, use tx-aware list_stream_tx so SSI
        // read-set recording is fused into this single scan (see read_collecting).
        let mut stream: DynBatchStream<'_> = if tx.is_some() {
            Box::pin(self.list_stream_tx(tx, batch_size))
        } else if let Some(pf) = pre_filter {
            Box::pin(self.list_stream_filtered(batch_size, pf))
        } else {
            Box::pin(self.list_stream(batch_size))
        };

        let mut records_scanned: u64 = 0;
        let mut skipped: usize = 0;
        let mut result: Vec<QueryRecord> = Vec::new();
        let mut has_next = false;
        let mut done = false;

        while let Some(batch_result) = stream.next().await {
            if done {
                break;
            }
            let batch = batch_result?;
            records_scanned += batch.len() as u64;

            for (_, cow) in &batch {
                // ── Id-keyed pass-through path (S-read) ─────────────────────
                if use_id_encoding {
                    match cow {
                        RecordCow::Borrowed(b) => {
                            let view = match shamir_types::record_view::RecordView::new(b) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            let passes = match filter_cb {
                                Some(cb) => cb.matches(&view, ctx),
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
                                    has_next = true;
                                    done = true;
                                    break;
                                }
                            }
                            let record = if id_is_all {
                                // SELECT * — verbatim stored bytes.
                                QueryRecord::IdBytes(ByteBuf::from(b.as_ref()))
                            } else {
                                // Plain projection — extract selected fields
                                // without de-interning.
                                let ids = id_projection_ids.as_deref().unwrap_or(&[]);
                                match record_view_to_id_msgpack(&view, ids) {
                                    Ok(bytes) => {
                                        QueryRecord::IdBytes(ByteBuf::from(bytes.as_ref()))
                                    }
                                    Err(_) => {
                                        QueryRecord::Direct(proj.project_value(&view, interner))
                                    }
                                }
                            };
                            result.push(record);
                        }
                        RecordCow::Owned(iv) => {
                            let passes = match filter_cb {
                                Some(cb) => cb.matches(iv, ctx),
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
                                    has_next = true;
                                    done = true;
                                    break;
                                }
                            }
                            // Owned: encode to bytes, then treat as Borrowed.
                            let bytes = iv.to_bytes().map_err(|e| DbError::Codec(e.to_string()))?;
                            let record = if id_is_all {
                                QueryRecord::IdBytes(ByteBuf::from(bytes.as_ref()))
                            } else {
                                match shamir_types::record_view::RecordView::new(&bytes) {
                                    Ok(view) => {
                                        let ids = id_projection_ids.as_deref().unwrap_or(&[]);
                                        match record_view_to_id_msgpack(&view, ids) {
                                            Ok(projected) => QueryRecord::IdBytes(ByteBuf::from(
                                                projected.as_ref(),
                                            )),
                                            Err(_) => QueryRecord::Direct(
                                                proj.project_value(iv, interner),
                                            ),
                                        }
                                    }
                                    Err(_) => QueryRecord::Direct(proj.project_value(iv, interner)),
                                }
                            };
                            result.push(record);
                        }
                    }
                    continue; // next record in this batch
                }

                // ── Name path (R5 de-intern) ─────────────────────────────────
                macro_rules! stream_row {
                    ($rec:expr) => {{
                        let passes = match filter_cb {
                            Some(cb) => cb.matches($rec, ctx),
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
                                has_next = true;
                                done = true;
                                break;
                            }
                        }
                        result.push(QueryRecord::Direct(proj.project_value($rec, interner)));
                    }};
                }

                match cow {
                    RecordCow::Borrowed(b) => {
                        let view = match shamir_types::record_view::RecordView::new(b) {
                            Ok(v) => v,
                            Err(_) => continue, // malformed row → skip
                        };
                        stream_row!(&view);
                    }
                    RecordCow::Owned(record) => {
                        stream_row!(record);
                    }
                }
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
            explain: None,
            skipped: false,
        })
    }

    /// V3.1 / P3 leaf 3.1 — filtered ANN execution path.
    ///
    /// Recognised shape: `And([VectorSimilarity{field,query,k,ef,oversample},
    /// ...residual-predicates])`. The planner compiled it into a
    /// [`FilteredVectorQuery`] (vector half + residual filter).
    ///
    /// Algorithm (post-filter with adaptive oversample-retry):
    /// 1. Resolve the oversample multiplier (`None` → 2× default, clamped ≥1×).
    /// 2. Compute `k′ = min(k × oversample, MAX_TOPK)`.
    /// 3. Loop:
    /// - a. ANN search for k′ candidates (tx-aware via `lookup_tx` so
    ///   in-tx staged vectors are visible).
    /// - b. Materialise candidate records and apply the residual predicate.
    /// - c. If ≥ k survivors → truncate to k, done.
    /// - d. If < k survivors AND k′ < MAX_TOPK → double k′ (clamped to
    ///   MAX_TOPK), retry.
    /// - e. If < k survivors AND k′ == MAX_TOPK → return what we have
    ///   (even if < k; the filter is too selective to fill k from the
    ///   available candidates).
    ///
    /// The result preserves ANN ranking order (nearest-first) among the
    /// survivors, which matches the bare-VectorSimilarity path's contract.
    ///
    /// Gate: this is a plain-SELECT path only (no GROUP BY / aggregates /
    /// DISTINCT / ORDER BY). Those clauses would require in-memory
    /// post-processing over the full survivor set and are left to the
    /// legacy paths — if a query carries them, the planner does NOT
    /// recognise the filtered-vector shape (the caller checks `fvq` shape
    /// but this method further gates on the query structure and falls
    /// through to `read_collecting` when needed).
    #[allow(clippy::too_many_arguments)]
    async fn read_filtered_vector_scan(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        fvq: &super::filtered_vector::FilteredVectorQuery,
        tx: Option<&shamir_tx::TxContext>,
        start: Instant,
    ) -> DbResult<QueryResult> {
        use crate::index2::backend::{IndexQuery, IndexResult};
        use crate::index2::vector::hnsw_adapter::MAX_TOPK;
        use crate::index2::vector::SearchOpts;

        // Resolve the vector backend by field path + "vector" kind.
        let field_path = crate::query::filter::eval::intern_field_path(&fvq.field, interner);
        let backend = match &field_path {
            Some(fp) => {
                self.index2_registry()
                    .find_by_field_and_kind(fp, "vector")
                    .await
            }
            None => None,
        };
        let backend = match backend {
            Some(b) => b,
            None => {
                // No vector index on this field — fall through to the legacy
                // full-scan path (VectorSimilarity compiles to FilterNode::True,
                // so the scan returns residual-matched rows unranked). This is
                // the pre-V3.1 behaviour for a filtered-vector query without
                // an index.
                return self
                    .read_fallback_no_vector_index(query, ctx, interner, tx, start)
                    .await;
            }
        };

        // Compile the residual predicate once (reused across retry iterations).
        let residual_cb: Option<FilterNode> =
            fvq.residual.as_ref().map(|f| compile_filter(f, interner));

        let k = fvq.k;
        let oversample = super::filtered_vector::resolve_oversample(fvq.oversample);
        let opts = SearchOpts {
            ef_search: fvq.ef_search,
            oversample: Some(oversample),
        };

        // Resolve staged vectors for tx-aware search.
        let table_token = self.table_token();
        let staged = tx.and_then(|t| t.staged_vectors_for(table_token));

        // ── V3.2 cost-based path selection ──────────────────────────────
        // Try to resolve the residual predicate against a secondary index to
        // get a candidate RID set. If successful, pick pre-filter or co-filter
        // based on cardinality; otherwise fall through to post-filter (V3.1).
        if let Some(ref residual) = fvq.residual {
            use crate::index2::vector::hnsw_adapter::{
                CO_FILTER_MAX_SELECTIVITY, PRE_FILTER_MAX_CANDIDATES,
            };
            use crate::index2::vector::vector_backend::VectorBackend;
            use crate::index2::vector::VectorAdapter as _;

            // Attempt to resolve candidate RIDs from secondary index (btree/functional).
            // IMPORTANT (C1): only enter fast-path when the index FULLY covers the
            // residual predicate. A partial cover (superset of matching RIDs) would
            // skip uncovered conjuncts and return wrong results.
            let candidate_rids: Option<Vec<RecordId>> = {
                // Try index2 path first (functional/fts/btree).
                // `try_plan_index2` only matches single-predicate shapes (Eq on
                // functional, FTS, bare Vector) — never multi-conjunct And. So a
                // successful Set result implies full coverage of the residual.
                let idx2_result = self.try_plan_index2(residual, interner).await;
                match idx2_result {
                    Some(IndexResult::Set(set)) => Some(set.into_iter().collect()),
                    _ => {
                        // Try legacy btree index scan. Only use when the index
                        // FULLY covers the residual (no leftover predicate).
                        if let Some((idx_name, lookup_sets, leftover)) =
                            self.try_plan_index_scan(residual, interner)
                        {
                            if leftover.is_some() {
                                // Partial coverage — uncovered conjuncts remain.
                                // Cannot use fast-path; fall through to post-filter.
                                None
                            } else {
                                let mut rids = Vec::new();
                                for values in &lookup_sets {
                                    if let Ok(ids) = self
                                        .index_manager_ref()
                                        .lookup_by_index(idx_name, values)
                                        .await
                                    {
                                        // Audit 1.5/3.2: `ids` is now
                                        // `Arc<[RecordId]>` (sorted slice) —
                                        // iterate the contiguous buffer.
                                        rids.extend(ids.iter().copied());
                                    }
                                }
                                if rids.is_empty() {
                                    None
                                } else {
                                    Some(rids)
                                }
                            }
                        } else {
                            None
                        }
                    }
                }
            };

            if let Some(candidates) = candidate_rids {
                let n_candidates = candidates.len();

                // Downcast the vector backend to access HnswAdapter.
                let vb_opt = backend.as_any().downcast_ref::<VectorBackend>();
                if let Some(vb) = vb_opt {
                    let adapter_arc = vb.adapter_arc();
                    if let Some(hnsw) = adapter_arc.as_hnsw_adapter() {
                        let total_live = hnsw.len();
                        let selectivity = if total_live > 0 {
                            n_candidates as f64 / total_live as f64
                        } else {
                            1.0
                        };

                        if n_candidates <= PRE_FILTER_MAX_CANDIDATES {
                            // PRE-FILTER: exact SIMD scoring over small candidate set.
                            let ranked = hnsw
                                .search_prefilter(&fvq.query, k, &candidates)
                                .await
                                .map_err(|e| DbError::Internal(e.to_string()))?;
                            // VR-5 (Б-5) — merge tx-staged vectors that pass the
                            // residual predicate so the pre-filter path sees the
                            // caller's own in-tx writes (read-your-own-writes),
                            // identical to the post-filter path.
                            let ranked = self
                                .merge_staged_filtered(
                                    hnsw,
                                    &fvq.query,
                                    k,
                                    ranked,
                                    staged,
                                    residual_cb.as_ref(),
                                    ctx,
                                    tx,
                                )
                                .await;
                            return self
                                .build_filtered_vector_result(
                                    query,
                                    interner,
                                    ctx,
                                    tx,
                                    start,
                                    &ranked,
                                    "pre_filter",
                                )
                                .await;
                        } else if selectivity <= CO_FILTER_MAX_SELECTIVITY {
                            // CO-FILTER: HNSW search_filter with allow-set.
                            let ranked = hnsw
                                .search_cofilter(&fvq.query, k, fvq.ef_search, &candidates)
                                .await
                                .map_err(|e| DbError::Internal(e.to_string()))?;
                            // VR-5 (Б-5) — same staged merge as pre-filter above.
                            let ranked = self
                                .merge_staged_filtered(
                                    hnsw,
                                    &fvq.query,
                                    k,
                                    ranked,
                                    staged,
                                    residual_cb.as_ref(),
                                    ctx,
                                    tx,
                                )
                                .await;
                            return self
                                .build_filtered_vector_result(
                                    query,
                                    interner,
                                    ctx,
                                    tx,
                                    start,
                                    &ranked,
                                    "co_filter",
                                )
                                .await;
                        }
                        // else: selectivity too high → fall through to post-filter
                    }
                }
            }
        }

        // Adaptive oversample-retry loop (POST-FILTER, V3.1).
        let mut k_prime = (((k as f32) * oversample).ceil() as u32)
            .max(k) // never below k
            .min(MAX_TOPK);
        let mut last_ranked: Vec<(RecordId, f32)>;
        // Survivors carry their already-resolved record bytes so the projection
        // below reuses them instead of re-fetching — the residual pass has
        // already read every candidate through `get_many_bytes_tx` (order is
        // ANN rank order, preserved by the byte fetch).
        let mut last_survivors: Vec<bytes::Bytes>;

        loop {
            let result = backend
                .lookup_tx(
                    table_token,
                    IndexQuery::Vector {
                        vec: fvq.query.clone(),
                        k: k_prime,
                        opts,
                    },
                    tx,
                    staged,
                )
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

            last_ranked = match result {
                IndexResult::Ranked(r) => r,
                IndexResult::Set(_) => {
                    // Vector backend returns Ranked; a Set is a contract
                    // violation. Fall back rather than panic.
                    return self
                        .read_fallback_no_vector_index(query, ctx, interner, tx, start)
                        .await;
                }
            };

            // Materialise candidate records and apply residual filter.
            // For tx-aware reads, staged records live in the tx's staging
            // store (write_set), NOT in the committed data store — so we
            // must read-through the staging store to resolve them.
            let candidates = last_ranked.len();
            let rids: Vec<RecordId> = last_ranked.iter().map(|(r, _)| *r).collect();
            let raw_records = self.get_many_bytes_tx(&rids, tx).await?;
            last_survivors = Vec::with_capacity(candidates);

            for maybe_bytes in raw_records.iter() {
                let bytes = match maybe_bytes {
                    Some(b) => b,
                    None => continue, // deleted/tombstoned — skip
                };
                let passes = match &residual_cb {
                    Some(cb) => {
                        // Evaluate the residual predicate on the record.
                        match shamir_types::record_view::RecordView::new(bytes) {
                            Ok(view) => cb.matches(&view, ctx),
                            Err(_) => continue, // malformed — skip
                        }
                    }
                    None => true,
                };
                if passes {
                    last_survivors.push(bytes.clone());
                }
            }

            // Got enough? Truncate to k and done.
            if last_survivors.len() >= k as usize {
                last_survivors.truncate(k as usize);
                break;
            }

            // Backend returned fewer candidates than we asked for → the HNSW
            // graph is exhausted; widening k′ cannot surface more. Stop with
            // what we have rather than spin identical lookups up to the cap.
            if candidates < k_prime as usize {
                break;
            }

            // Not enough. Can we widen k′?
            if k_prime >= MAX_TOPK {
                // Exhausted the cap — return what we have (< k).
                break;
            }

            // Double k′, clamp to MAX_TOPK, retry.
            k_prime = k_prime.saturating_mul(2).min(MAX_TOPK);
        }

        // Project survivors into query records — bytes already resolved in the
        // loop, so no second round-trip to the store/staging.
        let proj = exec::SelectProjection::new(&query.select, interner, ctx.scalars.clone());
        let mut records: Vec<QueryRecord> = Vec::with_capacity(last_survivors.len());
        for bytes in &last_survivors {
            let qv = match shamir_types::record_view::RecordView::new(bytes) {
                Ok(view) => proj.project_value(&view, interner),
                Err(_) => match InnerValue::from_bytes(bytes.clone()) {
                    Ok(iv) => match shamir_types::codecs::interned::inner_value_to_query_value(
                        &iv, interner,
                    ) {
                        Ok(q) => q,
                        Err(_) => continue,
                    },
                    Err(_) => continue,
                },
            };
            records.push(QueryRecord::Direct(qv));
        }

        let returned = records.len() as u64;
        let scanned = last_ranked.len() as u64;
        let elapsed = start.elapsed();

        Ok(QueryResult {
            records,
            stats: Some(QueryStats {
                index_used: Some("filtered_vector_scan".into()),
                records_scanned: scanned,
                records_returned: returned,
                execution_time_us: elapsed.as_micros() as u64,
            }),
            pagination: if query.pagination.is_none() {
                None
            } else {
                Some(PaginationInfo::compute(&query.pagination, Some(returned)))
            },
            value: None,
            explain: None,
            skipped: false,
        })
    }

    /// Fallback for a filtered-vector query that has NO vector index on the
    /// field. Routes to `read_collecting` / `read_streaming` so the residual
    /// predicates are still applied (VectorSimilarity compiles to
    /// `FilterNode::True` → all rows pass the vector conjunct, only the
    /// residual matters). This preserves pre-V3.1 behaviour.
    async fn read_fallback_no_vector_index(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        tx: Option<&shamir_tx::TxContext>,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let batch_size = shamir_tunables::store_defaults::FULL_SCAN_BATCH;
        let filter_cb: Option<Arc<FilterNode>> = query
            .r#where
            .as_ref()
            .map(|f| Arc::new(compile_filter(f, interner)));
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);
        let has_order = query.order_by.is_some();
        let has_distinct = query.select.distinct;
        let needs_full_collect = has_group_by || has_agg || has_order || has_distinct;
        if needs_full_collect {
            self.read_collecting(
                query,
                ctx,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                batch_size,
                start,
                tx,
            )
            .await
        } else if query.count_total {
            self.read_counting(
                query,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                ctx,
                batch_size,
                start,
                tx,
            )
            .await
        } else {
            self.read_streaming(
                query,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                ctx,
                batch_size,
                start,
                tx,
                ResultEncoding::Name,
            )
            .await
        }
    }

    /// VR-5 (Б-5) — Merge tx-staged vectors into the ranked output of the
    /// pre-filter / co-filter paths so they honour read-your-own-writes.
    ///
    /// Staged vectors live in `TxContext::staged_vectors` and are NOT in the
    /// committed HNSW graph, so `search_prefilter` / `search_cofilter` cannot
    /// see them. The post-filter path already sees staged rows (it resolves
    /// candidates via `backend.lookup_tx(..., staged)` which brute-force-merges
    /// them). To give the pre/co-filter paths identical visibility semantics,
    /// this helper:
    ///
    /// 1. Resolves the full record bytes for each staged vector (so the
    ///    residual predicate has the fields it needs, not just the embedding).
    /// 2. Applies the residual predicate via `FilterNode::matches` — staged
    ///    rows that do NOT match the residual are excluded (NOT "always
    ///    include"). This mirrors the post-filter residual pass.
    /// 3. Scores the residual-matching staged vectors brute-force via
    ///    `HnswAdapter::score_staged_candidates` (same `ShamirDist` kernel the
    ///    bare `search` path uses for its staged merge).
    /// 4. Merges into `ranked`, sorts by distance ascending, and truncates to
    ///    `k` — so the final result is the global top-k across committed +
    ///    staged, identical to what the post-filter path would return.
    ///
    /// Staged deletes are NOT consulted here: a staged delete removes the row
    /// from the tx's view, and `get_many_bytes_tx` (called by
    /// `build_filtered_vector_result` downstream) already returns `None` for a
    /// staged-removed RID, so even if a stale committed RID leaked into
    /// `ranked` it would be dropped at projection. Staged vector deletes
    /// target the graph side, which the pre/co-filter paths already respect
    /// (candidates come from the secondary index over committed rows).
    ///
    /// `staged` is `Option<&[(RecordId, Vec<f32>)]>` from
    /// `TxContext::staged_vectors_for(table_token)`. `residual_cb` is the
    /// compiled residual predicate (None when the filtered-vector query has
    /// no residual — then every staged vector passes).
    #[allow(clippy::too_many_arguments)]
    async fn merge_staged_filtered<'a>(
        &self,
        hnsw: &'a crate::index2::vector::hnsw_adapter::HnswAdapter,
        query: &[f32],
        k: u32,
        ranked: Vec<(RecordId, f32)>,
        staged: Option<&'a [(RecordId, Vec<f32>)]>,
        residual_cb: Option<&FilterNode>,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
    ) -> Vec<(RecordId, f32)> {
        // No staged vectors or no tx → nothing to merge (committed-only path).
        let staged = match (staged, tx) {
            (Some(s), _) if !s.is_empty() => s,
            _ => return ranked,
        };

        // #427 (VR-5, @sh adversarial-review finding) — an UPDATE-in-tx that
        // changes an ALREADY-committed row's embedding (without removing the
        // field) does NOT stage a vector delete (`stage_vector_deletes_on_update`
        // only stages a delete when the NEW record has no embedding at all —
        // `table_manager_tx_ops.rs`), so the row's OLD version is still in the
        // committed HNSW graph and can appear in `ranked` via
        // `search_prefilter`/`search_cofilter`. The SAME rid then also appears
        // in `staged` (the tx's new embedding). Without dedup, both versions
        // would be scored and merged — the same RecordId twice in `ranked`,
        // wasting a top-k slot and violating the "no duplicate rids" contract.
        // The staged (newer, tx-visible) version must win: drop any `ranked`
        // entry whose rid is ALSO staged, before merging the staged score in.
        let staged_rid_set: shamir_collections::TFxSet<RecordId> =
            staged.iter().map(|(r, _)| *r).collect();
        let mut ranked: Vec<(RecordId, f32)> = ranked
            .into_iter()
            .filter(|(rid, _)| !staged_rid_set.contains(rid))
            .collect();

        // Resolve the full record bytes for each staged vector so the residual
        // predicate has access to every field, not just the embedding.
        let staged_rids: Vec<RecordId> = staged.iter().map(|(r, _)| *r).collect();
        let staged_bytes = match self.get_many_bytes_tx(&staged_rids, tx).await {
            Ok(b) => b,
            // On read failure we MUST NOT silently drop staged rows (that would
            // be a correctness regression); nor can we safely include them
            // unscored. Return the (already de-duplicated) committed-only
            // ranked set — the read path is best-effort here.
            Err(e) => {
                log::warn!(
                    "merge_staged_filtered: get_many_bytes_tx failed for {} staged \
                     rid(s), falling back to committed-only filtered-ANN results \
                     (read-your-own-writes degraded for this query): {e}",
                    staged_rids.len()
                );
                return ranked;
            }
        };

        // Apply the residual predicate to each staged record; collect the
        // vectors that pass into a residual-matching staged set.
        let mut passing: Vec<(RecordId, Vec<f32>)> = Vec::with_capacity(staged.len());
        for (entry, maybe_bytes) in staged.iter().zip(staged_bytes.iter()) {
            let bytes = match maybe_bytes {
                Some(b) => b,
                None => continue, // staged remove / unreadable — skip
            };
            let passes = match residual_cb {
                None => true,
                Some(cb) => match shamir_types::record_view::RecordView::new(bytes) {
                    Ok(view) => cb.matches(&view, ctx),
                    Err(_) => continue, // malformed — skip
                },
            };
            if passes {
                passing.push(entry.clone());
            }
        }

        if passing.is_empty() {
            return ranked;
        }

        // Score the residual-matching staged vectors brute-force and merge.
        let scored = match hnsw.score_staged_candidates(query, k, &passing).await {
            Ok(s) => s,
            Err(_) => return ranked,
        };
        ranked.extend(scored);
        ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(k as usize);
        ranked
    }

    /// Build a `QueryResult` from pre-filter / co-filter ranked output.
    ///
    /// V3.2: shared helper for pre-filter and co-filter paths. Takes the
    /// ranked `(RecordId, f32)` pairs, resolves them to record bytes,
    /// projects, and returns the QueryResult with the given `index_tag`.
    #[allow(clippy::too_many_arguments)]
    async fn build_filtered_vector_result(
        &self,
        query: &ReadQuery,
        interner: &Interner,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
        start: Instant,
        ranked: &[(RecordId, f32)],
        index_tag: &str,
    ) -> DbResult<QueryResult> {
        let rids: Vec<RecordId> = ranked.iter().map(|(r, _)| *r).collect();
        let raw_records = self.get_many_bytes_tx(&rids, tx).await?;
        let proj = exec::SelectProjection::new(&query.select, interner, ctx.scalars.clone());
        let mut records: Vec<QueryRecord> = Vec::with_capacity(ranked.len());
        for maybe_bytes in raw_records.iter() {
            let bytes = match maybe_bytes {
                Some(b) => b,
                None => continue,
            };
            let qv = match shamir_types::record_view::RecordView::new(bytes) {
                Ok(view) => proj.project_value(&view, interner),
                Err(_) => match InnerValue::from_bytes(bytes.clone()) {
                    Ok(iv) => {
                        match shamir_types::codecs::interned::inner_value_to_query_value(
                            &iv, interner,
                        ) {
                            Ok(q) => q,
                            Err(_) => continue,
                        }
                    }
                    Err(_) => continue,
                },
            };
            records.push(QueryRecord::Direct(qv));
        }
        let returned = records.len() as u64;
        let scanned = ranked.len() as u64;
        let elapsed = start.elapsed();
        Ok(QueryResult {
            records,
            stats: Some(QueryStats {
                index_used: Some(index_tag.into()),
                records_scanned: scanned,
                records_returned: returned,
                execution_time_us: elapsed.as_micros() as u64,
            }),
            pagination: if query.pagination.is_none() {
                None
            } else {
                Some(PaginationInfo::compute(&query.pagination, Some(returned)))
            },
            value: None,
            explain: None,
            skipped: false,
        })
    }

    /// Build an [`ExplainPlan`] by running only the planner decision tree
    /// (the same cascade as `read_impl`) but without materialising any rows.
    fn build_explain_plan(&self, query: &ReadQuery, interner: &Interner) -> ExplainPlan {
        // 1. Counter shortcut: count(*) without WHERE.
        if query.r#where.is_none()
            && query.group_by.is_none()
            && query.order_by.is_none()
            && !query.select.distinct
            && !query.count_total
            && query.pagination.is_none()
            && query.select.items.len() == 1
        {
            if let SelectItem::CountAll { .. } = &query.select.items[0] {
                return ExplainPlan {
                    plan_type: PlanType::CounterShortcut,
                    index_used: Some("__record_counter__".into()),
                    estimated_rows: None,
                };
            }
            // MIN aggregate via sorted index.
            if let SelectItem::Aggregate {
                func: shamir_query_types::read::AggFunc::Min,
                field: shamir_query_types::read::AggregateField::Field(path),
                ..
            } = &query.select.items[0]
            {
                if let Some(fp) = intern_field_path(path, interner) {
                    if let Some(def) = self.sorted_indexes().find_by_field(&fp) {
                        return ExplainPlan {
                            plan_type: PlanType::MinMaxIndex,
                            index_used: Some(format!("sorted_idx_{}_min", def.name_interned)),
                            estimated_rows: Some(1),
                        };
                    }
                }
            }
        }

        // 1b. V3.1: filtered ANN (And[VectorSimilarity, ...residual]).
        if let Some(ref filter) = query.r#where {
            if super::filtered_vector::try_extract_filtered_vector_query(filter).is_some() {
                return ExplainPlan {
                    plan_type: PlanType::Index2,
                    index_used: Some("filtered_vector_scan".into()),
                    estimated_rows: None,
                };
            }
        }

        // 2. Index2 (FTS / Functional / Vector).
        if let Some(ref filter) = query.r#where {
            if !self.index2_registry().is_empty() {
                // Check FTS / Vector / Computed shapes without actually running lookup.
                let might_use_index2 = matches!(
                    filter,
                    crate::query::filter::Filter::Fts { .. }
                        | crate::query::filter::Filter::VectorSimilarity { .. }
                        | crate::query::filter::Filter::Computed { .. }
                );
                if might_use_index2 {
                    return ExplainPlan {
                        plan_type: PlanType::Index2,
                        index_used: Some("index2".into()),
                        estimated_rows: None,
                    };
                }
            }
        }

        // 3. BTree index scan (Eq / In / And).
        if let Some(ref filter) = query.r#where {
            if let Some((idx_name, _lookup_sets, _residual)) =
                self.try_plan_index_scan(filter, interner)
            {
                return ExplainPlan {
                    plan_type: PlanType::IndexScan,
                    index_used: Some(format!("idx_{idx_name}")),
                    estimated_rows: None,
                };
            }
        }

        // 3b. MAX aggregate via sorted index.
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
                ..
            } = &query.select.items[0]
            {
                if let Some(fp) = intern_field_path(path, interner) {
                    if let Some(def) = self.sorted_indexes().find_by_field(&fp) {
                        return ExplainPlan {
                            plan_type: PlanType::MinMaxIndex,
                            index_used: Some(format!("sorted_idx_{}_max", def.name_interned)),
                            estimated_rows: Some(1),
                        };
                    }
                }
            }
        }

        // 4. Keyset seek.
        if let Some((idx_name, _encoded_key, _after_id, _limit, _direction)) =
            self.try_plan_keyset_seek(query, interner)
        {
            return ExplainPlan {
                plan_type: PlanType::KeysetSeek,
                index_used: Some(format!("sorted_idx_{idx_name}")),
                estimated_rows: None,
            };
        }

        // 5. ORDER BY + LIMIT fast path.
        if let Some((idx_name, take, skip, _direction)) =
            self.try_plan_order_limit_fast_path(query, interner)
        {
            return ExplainPlan {
                plan_type: PlanType::OrderLimitFast,
                index_used: Some(format!("sorted_idx_{idx_name}")),
                estimated_rows: Some((skip + take) as u64),
            };
        }

        // 6. Sorted-index range scan (top-level).
        if let Some(ref filter) = query.r#where {
            if let Some((idx_name, _lo, _hi, _residual)) =
                self.try_plan_sorted_index_scan(filter, interner)
            {
                return ExplainPlan {
                    plan_type: PlanType::SortedIndexScan,
                    index_used: Some(format!("sorted_idx_{idx_name}")),
                    estimated_rows: None,
                };
            }
        }

        // 7. AND-range extraction.
        if let Some(ref filter) = query.r#where {
            if let Some((idx_name, _lo, _hi, _residual)) =
                self.try_plan_and_range_index_scan(filter, interner)
            {
                return ExplainPlan {
                    plan_type: PlanType::AndRangeIndexScan,
                    index_used: Some(format!("sorted_idx_{idx_name}")),
                    estimated_rows: None,
                };
            }
        }

        // 8. Full scan (fallback).
        ExplainPlan {
            plan_type: PlanType::FullScan,
            index_used: None,
            estimated_rows: None,
        }
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
pub(super) fn try_project_page_only(
    query: &ReadQuery,
    matched: &[(RecordId, InnerValue)],
    interner: &Interner,
    scalars: ScalarResolver,
) -> Option<(Vec<crate::query::read::QueryRecord>, Option<PaginationInfo>)> {
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

    let proj = exec::SelectProjection::new(&query.select, interner, scalars.clone());
    let mut paged: Vec<crate::query::read::QueryRecord> = Vec::with_capacity(page_slice.len());
    for (_, record) in page_slice {
        paged.push(crate::query::read::QueryRecord::Direct(
            proj.project_value(record, interner),
        ));
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

/// Byte-level twin of [`try_project_page_only`] — works on raw `Bytes`
/// instead of decoded `InnerValue`. Each row is wrapped in a zero-copy
/// `RecordView` for projection. Bare-scalar / non-map records where
/// `RecordView::new` fails are decoded via `InnerValue::from_bytes` as a
/// fallback (records are NEVER silently dropped).
pub(super) fn try_project_page_only_bytes(
    query: &ReadQuery,
    matched: &[(RecordId, Bytes)],
    interner: &Interner,
    scalars: ScalarResolver,
) -> Option<(Vec<crate::query::read::QueryRecord>, Option<PaginationInfo>)> {
    // Same eligibility gate as the InnerValue variant.
    if query.order_by.is_some()
        || query.group_by.is_some()
        || query.select.distinct
        || exec::has_aggregates(&query.select)
    {
        return None;
    }
    if query.pagination.is_none() && !query.count_total {
        return None;
    }
    let (skip_u64, take_u64) = query.pagination.resolve();
    let take = take_u64? as usize;
    let skip = skip_u64 as usize;

    let total_matches = matched.len();
    let total_u64 = total_matches as u64;

    let page_start = skip.min(total_matches);
    let page_end = skip.saturating_add(take).min(total_matches);
    let page_slice = &matched[page_start..page_end];

    let proj = exec::SelectProjection::new(&query.select, interner, scalars.clone());
    let mut paged: Vec<crate::query::read::QueryRecord> = Vec::with_capacity(page_slice.len());
    for (_, bytes) in page_slice {
        let qv = match shamir_types::record_view::RecordView::new(bytes) {
            Ok(view) => proj.project_value(&view, interner),
            Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                Ok(iv) => proj.project_value(&iv, interner),
                Err(_) => continue,
            },
        };
        paged.push(crate::query::read::QueryRecord::Direct(qv));
    }

    let pagination = if query.pagination.is_none() && query.count_total {
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

/// Byte-level twin of [`exec::apply_select_value`] — projects raw `Bytes`
/// rows via zero-copy `RecordView` instead of decoded `InnerValue`.
/// Bare-scalar / non-map records fall back to `InnerValue::from_bytes`.
pub(super) fn apply_select_value_bytes(
    matched: &[(RecordId, Bytes)],
    select: &crate::query::read::Select,
    interner: &Interner,
    scalars: ScalarResolver,
) -> Vec<QueryValue> {
    let proj = exec::SelectProjection::new(select, interner, scalars.clone());
    matched
        .iter()
        .map(
            |(_, bytes)| match shamir_types::record_view::RecordView::new(bytes) {
                Ok(view) => proj.project_value(&view, interner),
                Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                    Ok(iv) => proj.project_value(&iv, interner),
                    Err(_) => QueryValue::Null,
                },
            },
        )
        .collect()
}
