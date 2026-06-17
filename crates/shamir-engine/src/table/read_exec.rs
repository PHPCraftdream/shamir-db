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
    exec, AggregateField, GroupBy, PaginationInfo, QueryResult, QueryStats, ReadQuery, Select,
    SelectItem, Temporal,
};
use shamir_collections::TFxSet;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

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
// AGG #54 — per-row tree decode prune on the aggregate / GROUP BY read path.
//
// Today every row in a GROUP BY / aggregate query is fully decoded to an
// `InnerValue` tree even when the query references only a few fields of a wide
// record. The helpers below compute the set of referenced *top-level* interned
// field ids ONCE per query, gate the optimisation so it only applies when the
// set is provably complete + concrete (falling back to the full
// `InnerValue::from_bytes` decode otherwise), and build a pruned
// `InnerValue::Map` containing only those referenced top-level subtrees. The
// aggregate engine (aggregate.rs) is unchanged: for every referenced path the
// pruned tree is identical to the full tree, so `resolve_field_ref`,
// `group_key_item`, Min/Max borrows and funclib all behave byte-identically.
// ============================================================================

/// Compute the set of referenced TOP-LEVEL interned field ids a query needs,
/// or `None` when the prune must FALL BACK to the full decode.
///
/// The set is the FIRST path segment of every GROUP BY field, every concrete
/// `AggregateField::Field(path)` (under both `SelectItem::Aggregate` and
/// `AggregateFn`), and every `SelectItem::Field` projection. Keeping a
/// referenced top-level field means keeping its WHOLE subtree, so nested paths
/// (`a.b.c` → keep top-level `a`) resolve correctly afterwards.
///
/// **GATE — returns `None` (fall back to full decode) when the query contains
/// any of:**
/// - `SelectItem::All` (SELECT *) — reads every field;
/// - `AggregateField::All` under a record-touching `Aggregate`/`AggregateFn`
///   (NOT `SelectItem::CountAll`, which never reads a record);
/// - `SelectItem::Function` / `$fn` scalar items — may read arbitrary fields;
/// - `SelectItem::Expression` — may read arbitrary fields;
/// - a referenced first-segment that is not interned (`get_ind` miss) — the
///   set would be incomplete, so we fall back rather than silently drop it.
///
/// `group_by` is `Option<&GroupBy>`; when present its field paths contribute
/// their first segments (and a HAVING whose referenced fields aren't already
/// covered is conservatively treated via the select-list gate above — HAVING
/// itself runs post-aggregate on the result object, so it does not read raw
/// records, but its field coverage is only knowable through the select list).
pub(super) fn collect_referenced_top_ids(
    select: &Select,
    group_by: Option<&GroupBy>,
    interner: &Interner,
) -> Option<TFxSet<u64>> {
    // Upper bound on the number of referenced ids: group fields + one per item.
    let cap = select.items.len() + group_by.map(|g| g.fields.len()).unwrap_or(0);
    let mut ids: TFxSet<u64> = shamir_collections::new_fx_set_wc(cap);

    // Helper: intern the first segment of a path and insert; return false on miss.
    let mut push_first = |path: &[String]| -> bool {
        match path.first() {
            Some(seg) => match interner.get_ind(seg) {
                Some(k) => {
                    ids.insert(k.id());
                    true
                }
                None => false,
            },
            None => true,
        }
    };

    for item in &select.items {
        match item {
            // FALL-BACK triggers — these read arbitrary / all fields.
            SelectItem::All => return None,
            SelectItem::Function { .. } => return None,
            SelectItem::Expression { .. } => return None,
            // CountAll never touches records — contributes no field id and is
            // safe to keep alongside pruned items.
            SelectItem::CountAll { .. } => {}
            SelectItem::Field { path, .. } => {
                if !push_first(path) {
                    return None;
                }
            }
            SelectItem::Aggregate { field, .. } => match field {
                AggregateField::All => return None,
                AggregateField::Field(p) => {
                    if !push_first(p) {
                        return None;
                    }
                }
            },
            SelectItem::AggregateFn { field, .. } => match field {
                AggregateField::All => return None,
                AggregateField::Field(p) => {
                    if !push_first(p) {
                        return None;
                    }
                }
            },
        }
    }

    // GROUP BY field paths — their first segments must be interned too.
    if let Some(gb) = group_by {
        for f in &gb.fields {
            if !push_first(f) {
                return None;
            }
        }
    }

    Some(ids)
}

/// Build a pruned `InnerValue::Map` containing ONLY the referenced top-level
/// fields, by extracting each field's value bytes from the lens and decoding
/// just that subtree. Unreferenced wide fields are never decoded.
///
/// For every referenced path the resulting tree is byte-identical to the one
/// `InnerValue::from_bytes(full_record_bytes)` would have produced: nested
/// map keys decode back to `InternerKey` (the storage codec serialises them
/// that way), so `resolve_field_ref(&pruned, path)` returns the same
/// `&InnerValue` as on the full tree.
///
/// Fields in `top_ids` that are absent from the record are simply omitted
/// (matching `resolve_field_ref`'s `None` on miss — no spurious `Null` insert).
pub(super) fn prune_to_inner(
    view: &shamir_types::record_view::RecordView<'_>,
    top_ids: &TFxSet<u64>,
) -> InnerValue {
    let mut map = new_map_wc(top_ids.len());
    for &id in top_ids {
        let key = InternerKey::new(id);
        if let Some(bytes) = view.field_value_bytes(key.clone()) {
            // A miss on decode means malformed bytes for a present field;
            // mirror the full-decode path's behaviour by skipping (the row was
            // already accepted as well-formed enough to build a RecordView, so
            // this branch is pathological — but never panic).
            if let Ok(sub) = InnerValue::from_bytes(bytes) {
                map.insert(key, sub);
            }
        }
    }
    InnerValue::Map(map)
}

impl TableManager {
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
        self.read_impl(query, ctx, None).await
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
        self.read_impl(query, ctx, tx).await
    }

    async fn read_impl(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
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
                                records: vec![crate::query::read::QueryRecord::Json(
                                    serde_json::Value::Object(obj),
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
                    records: vec![crate::query::read::QueryRecord::Json(
                        serde_json::Value::Object(obj),
                    )],
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
                    // Preserve full match count for pagination metadata BEFORE
                    // any page slice — clients rely on count_total for UI.
                    let count_total = rids_vec.len() as u64;

                    // Opt #5 (1000×-class): push pagination into index2 path.
                    // index2 returns a pre-filtered, pre-ranked RID list with no
                    // residual predicate, so it is safe to slice [skip..skip+take]
                    // before calling get_many. Gate: finite LIMIT must be present.
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

                    let inner_records = self.get_many(rids_slice).await?;
                    let mut records = Vec::with_capacity(inner_records.len());
                    for inner in inner_records.into_iter().flatten() {
                        if let Ok(qv) = shamir_types::codecs::interned::inner_value_to_query_value(
                            &inner, interner,
                        ) {
                            records.push(crate::query::read::QueryRecord::Direct(qv));
                        }
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
                            records: vec![crate::query::read::QueryRecord::Json(
                                serde_json::Value::Object(obj),
                            )],
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
                                records: vec![crate::query::read::QueryRecord::Json(
                                    serde_json::Value::Object(obj),
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

        let filter_cb: Option<Arc<FilterNode>> = query
            .r#where
            .as_ref()
            .map(|f| Arc::new(compile_filter(f, interner)));

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
            )
            .await
        }
    }

    /// Collecting path: streams batches, accumulates what's needed, then applies
    /// GROUP BY / aggregates / ORDER BY / DISTINCT / PAGINATION.
    ///
    /// For GROUP BY / aggregates — accumulates raw InnerValues (needed for
    /// field extraction). For plain SELECT + ORDER BY / DISTINCT — accumulates
    /// already-projected JSON values (smaller footprint than raw records).
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
        let prune_ids: Option<TFxSet<u64>> = if needs_raw {
            collect_referenced_top_ids(&query.select, query.group_by.as_ref(), interner)
        } else {
            None
        };

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

        // Two accumulation modes — raw InnerValues or projected QueryRecord
        let mut raw_acc: Vec<(RecordId, InnerValue)> = Vec::new();
        let mut rec_acc: Vec<crate::query::read::QueryRecord> = Vec::new();
        let proj = if !needs_raw {
            Some(exec::SelectProjection::new(&query.select, interner))
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
                                // AGG #54 — when the referenced-id gate
                                // succeeded, decode ONLY the referenced
                                // top-level subtrees (wide unreferenced fields
                                // are never decoded). The pruned tree lives in
                                // `raw_acc` for the whole group walk, so
                                // Min/Max's `&'a InnerValue` borrows are
                                // satisfied identically to the full-decode
                                // path. When the gate fell back (`None`), do
                                // the original full `InnerValue::from_bytes`.
                                let inner = match prune_ids.as_ref() {
                                    Some(ids) => prune_to_inner(&view, ids),
                                    None => InnerValue::from_bytes(b)
                                        .map_err(|e| DbError::Codec(e.to_string()))?,
                                };
                                raw_acc.push((id, inner));
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
                                raw_acc.push((id, record));
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

        let result = if has_group_by {
            let group_by = query.group_by.as_ref().unwrap();
            exec::apply_group_by(&raw_acc, group_by, &query.select, interner, ctx)
                .into_iter()
                .map(crate::query::read::QueryRecord::Json)
                .collect()
        } else if has_agg {
            exec::apply_aggregate_all(&raw_acc, &query.select, interner)
                .into_iter()
                .map(crate::query::read::QueryRecord::Json)
                .collect()
        } else {
            rec_acc
        };

        // apply_distinct / apply_order_by / apply_pagination operate on
        // Vec<serde_json::Value>.  Convert once for these post-processing
        // steps; the hot streaming/counting paths avoid this conversion
        // entirely.
        let mut json_result: Vec<serde_json::Value> =
            result.into_iter().map(serde_json::Value::from).collect();

        if query.select.distinct {
            json_result = exec::apply_distinct(json_result);
        }
        if let Some(ref order_by) = query.order_by {
            exec::apply_order_by(&mut json_result, order_by);
        }

        let (json_records, pagination) =
            exec::apply_pagination(json_result, &query.pagination, query.count_total);

        let elapsed = start.elapsed();
        let records_returned = json_records.len() as u64;
        let records: Vec<crate::query::read::QueryRecord> = json_records
            .into_iter()
            .map(crate::query::read::QueryRecord::Json)
            .collect();

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

        let proj = exec::SelectProjection::new(&query.select, interner);

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
    ) -> DbResult<QueryResult> {
        let (skip, take) = query.pagination.resolve();
        let skip = skip as usize;
        let limit = take.map(|t| t as usize);

        let proj = exec::SelectProjection::new(&query.select, interner);

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
        let mut result: Vec<crate::query::read::QueryRecord> = Vec::new();
        let mut has_next = false;
        let mut done = false;

        while let Some(batch_result) = stream.next().await {
            if done {
                break;
            }
            let batch = batch_result?;
            records_scanned += batch.len() as u64;

            for (_, cow) in &batch {
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
                        result.push(crate::query::read::QueryRecord::Direct(
                            proj.project_value($rec, interner),
                        ));
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
pub(super) fn try_project_page_only(
    query: &ReadQuery,
    matched: &[(RecordId, InnerValue)],
    interner: &Interner,
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

    let proj = exec::SelectProjection::new(&query.select, interner);
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
