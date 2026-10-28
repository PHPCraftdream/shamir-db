//! Read query execution on TableManager.
//!
//! Implements read(), index scan planning, and read execution strategies
//! (collecting, counting, streaming) for TableManager.

use std::time::Instant;

use crate::query::filter::eval::intern_field_path;
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{
    exec, GroupBy, QueryResult, QueryStats, ReadQuery, Select, SelectItem, Temporal,
};
use crate::table::read::dispatch::explain::contains_vector_similarity;
use crate::table::read::vector::filtered_vector::{
    snapshot_vector_query, try_extract_filtered_vector_query,
};
use crate::table::support::storage::record_cow::RecordCow;
use crate::table::table_manager::TableManager;

use shamir_query_types::batch::ResultEncoding;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

/// Boxed, `Send`-able stream of decoded record batches used by the three scan
/// execution paths (`read_collecting`, `read_counting`, `read_streaming`).
pub(in crate::table::read) type DynBatchStream<'a> = std::pin::Pin<
    Box<
        dyn futures::Stream<Item = shamir_storage::error::DbResult<Vec<(RecordId, RecordCow)>>>
            + Send
            + 'a,
    >,
>;

// ============================================================================
// FG-2 read-side version reporting (`with_version`).
//
// `ReadQuery::with_version == true` asks the engine to populate
// `QueryResult::versions` — a `Vec<u64>` index-aligned with `records`, where
// `versions[i]` is the canonical committed version of `records[i]`'s row.
//
// Source: `MvccStore::version_of(key)` — the SAME accessor SSI read-set
// validation already uses. Non-MVCC tables have no version authority, so
// `versions` stays `None` for them even when the client asks (the wire field
// is documented as opt-in assistance, never a correctness contract).
//
// Helper below is called on read paths where each output row maps 1:1 to
// exactly one source row whose `RecordId` (and therefore whose version) is
// known: order-preserving scans, AND the plain-ORDER BY path in
// `read_collecting` which threads each row's `RecordId` through the sort so
// the version array stays aligned after the reorder (F-7, #797).
//
// GROUP BY, aggregates, and DISTINCT COLLAPSE many source rows into one
// output row, so no single version applies to the collapsed result —
// `read_collecting` now REJECTS `with_version` for those combinations at
// request time (a hard `DbError::Validation`) rather than silently returning
// `versions = None`. ORDER BY alone does not collapse row identity (it only
// reorders), so it is permitted and the ids are threaded through the sort.
// ============================================================================

/// Build the `QueryResult::versions` array for `ids` when the client asked
/// for per-record versions. Returns `None` when:
///   - the client did not ask (`with_version == false`), or
///   - this table has no MVCC backing store (no version authority).
///
/// Otherwise returns `Some(Vec<u64>)` with `version_of(id)` per input id,
/// in the same order as `ids`.
pub(in crate::table::read) fn collect_versions(
    with_version: bool,
    mvcc: Option<&std::sync::Arc<shamir_tx::MvccStore>>,
    ids: &[RecordId],
) -> Option<Vec<u64>> {
    if !with_version {
        return None;
    }
    let mvcc = mvcc?;
    Some(
        ids.iter()
            .map(|id| mvcc.version_of(id.as_bytes()))
            .collect(),
    )
}

/// Compute the `versions` array from a matched `[(RecordId, Bytes)]` list,
/// honouring the query's pagination slice and ORDER-BY gate.
///
/// Returns `None` when:
///   - `with_version == false`, or
///   - no MvccStore, or
///   - ORDER BY is present (reorder breaks id↔version alignment — documented
///     exception per the brief).
///
/// When pagination is set, replicates `apply_pagination`'s `[skip..skip+take]`
/// slice on the matched ids so the version array stays aligned with the paged
/// records.
pub(in crate::table::read) fn versions_from_matched(
    query: &ReadQuery,
    mvcc: Option<&std::sync::Arc<shamir_tx::MvccStore>>,
    matched: &[(RecordId, bytes::Bytes)],
) -> Option<Vec<u64>> {
    if !query.with_version {
        return None;
    }
    let mvcc = mvcc?;
    // ORDER BY reorders rows after projection, breaking id↔version alignment.
    if query.order_by.is_some() {
        return None;
    }
    let (skip, take) = query.pagination.resolve();
    let skip = skip as usize;
    let take = take.map(|t| t as usize).unwrap_or(usize::MAX);
    let ids: Vec<RecordId> = matched
        .iter()
        .skip(skip)
        .take(take)
        .map(|(id, _)| *id)
        .collect();
    Some(
        ids.iter()
            .map(|id| mvcc.version_of(id.as_bytes()))
            .collect(),
    )
}

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
pub(in crate::table::read) fn is_select_simple(
    select: &Select,
    group_by: Option<&GroupBy>,
) -> bool {
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
pub(in crate::table::read) fn is_select_all(select: &Select) -> bool {
    select.items.is_empty() || select.items.iter().any(|i| matches!(i, SelectItem::All))
}

/// Intern the top-level field name (last path segment) of each plain
/// `SelectItem::Field` item and return the resulting interned-key list.
/// Called only when `is_select_simple` is true and `is_select_all` is false.
///
/// Returns `None` when any path is un-internable (miss in the interner) —
/// that field does not exist in this table so the projection would silently
/// drop it; we fall back to the Name path to preserve behaviour parity.
pub(in crate::table::read) fn intern_simple_projection_ids_with_context(
    select: &Select,
    ctx: &FilterContext<'_>,
) -> Option<Vec<InternerKey>> {
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
            let k = ctx.resolve_interner_id(key).map(InternerKey::new)?;
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

    /// Like `read` but honours a client-requested [`ResultEncoding`].
    ///
    /// When `encoding == Id` and the query is "simple" (SELECT * or plain
    /// field projection, no GROUP BY / aggregates / aliases / computed), rows
    /// are returned as [`QueryRecord::IdBytes`](crate::query::read::QueryRecord::IdBytes) — raw id-keyed storage msgpack,
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

    /// tx-aware variant of `read` used by `read_tx` to fuse the SSI
    /// read-set recording pass into the single scan that emits rows.
    /// For the full-scan fallback the three sub-methods use tx-aware streams
    /// so no second scan is needed. For index/shortcut paths, predicate-level
    /// SSI recording (already installed by `read_tx` before this call) is
    /// sufficient.
    pub(in crate::table) async fn read_for_tx(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<QueryResult> {
        self.read_impl(query, ctx, tx, ResultEncoding::Name).await
    }

    /// Like `read_for_tx` but honours a client-requested [`ResultEncoding`].
    pub(in crate::table) async fn read_for_tx_with_encoding(
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
        // Attach the interactive-tx overlay snapshot unless the caller already
        // did. Selected inline: no extra async layer on the read path.
        let overlay_ctx = match tx {
            Some(tx_ref)
                if !tx_ref.implicit
                    && !tx_ref.interner_overlay.is_empty()
                    && ctx.interner_overlay_ids.is_none() =>
            {
                Some(ctx.clone_with_read_overlay(&tx_ref.interner_overlay))
            }
            _ => None,
        };
        let ctx = overlay_ctx.as_ref().unwrap_or(ctx);
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
            return Ok(QueryResult::with_explain(plan));
        }

        // F-7 (#797): `with_version` asks for a per-record version array that
        // is only well-defined when each output row maps 1:1 to exactly one
        // source row. GROUP BY, aggregates, and DISTINCT all COLLAPSE many
        // source rows into one output row, so no single version applies to the
        // collapsed result — reject these combinations at request time rather
        // than silently returning `versions: None` (the earlier documented
        // FG-2 tradeoff). Plain ORDER BY (no collapse) is permitted and handled
        // inside `read_collecting` by carrying each row's RecordId through the
        // sort.
        //
        // Placed here (before the count(*) / min aggregate shortcuts and the
        // index/vector fast paths) so EVERY aggregate shape — including the
        // O(1) `count(*)` shortcut, which would otherwise silently return
        // `versions: None` — gets the same hard, clear error.
        if query.with_version
            && (query.group_by.is_some()
                || exec::has_aggregates(&query.select)
                || query.select.distinct)
        {
            return Err(DbError::Validation(
                "with_version is not supported with GROUP BY, aggregates, or DISTINCT: \
                 no single version applies to a collapsed row"
                    .to_string(),
            ));
        }

        // Latest-state counters/indexes cannot answer a snapshot + staged overlay.
        // Snapshot/Serializable reads pay O(N) scans (and sorting when needed);
        // Pessimistic and non-tx reads retain the latest-committed shortcuts.
        if tx.is_some_and(|t| {
            matches!(
                t.isolation,
                shamir_tx::IsolationLevel::Snapshot | shamir_tx::IsolationLevel::Serializable
            )
        }) {
            if let Some(fvq) = query.r#where.as_ref().and_then(snapshot_vector_query) {
                // The live ANN graph cannot be certified complete at a snapshot (graph
                // promotion follows publication and direct writes update the graph after
                // MVCC), so snapshot vector reads rank exactly: O(N*d) over the snapshot.
                return Box::pin(self.read_snapshot_vector(query, ctx, interner, &fvq, tx, start))
                    .await;
            }
            if !query
                .r#where
                .as_ref()
                .is_some_and(contains_vector_similarity)
            {
                if query.pagination.keyset().is_some()
                    || query.order_by.as_ref().is_some_and(|order| {
                        order
                            .items
                            .iter()
                            .any(|item| item.field.len() == 1 && item.field[0] == "_id")
                    })
                {
                    return self
                        .read_snapshot_keyset(query, ctx, interner, tx, start)
                        .await;
                }
                return self
                    .read_scan(query, ctx, interner, tx, encoding, batch_size, start)
                    .await;
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
                input: None,
                alias,
                ..
            } = &query.select.items[0]
            {
                if let Some(field_path) = intern_field_path(path, interner) {
                    // F-72 (#899, P0): state-filtered — a `Building` sorted
                    // index (CREATE INDEX backfill in flight) must fall
                    // through to the full scan below, not answer MIN from a
                    // half-populated index.
                    // P0-3a (#1037): IndexDrainInProgress also falls through.
                    if let Some(def) = self.sorted_indexes().find_by_field_ready(&field_path) {
                        match self.sorted_indexes().lookup_min(def.name_interned).await {
                            Err(DbError::IndexDrainInProgress(_)) => {
                                // Index is in drain window — fall through to full scan.
                            }
                            Err(e) => return Err(e),
                            Ok(Some(id)) => {
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
                                    .unwrap_or_else(|| {
                                        path.last().map(|s| s.as_str()).unwrap_or("min")
                                    })
                                    .to_string();
                                let mut obj = new_map_wc(1);
                                obj.insert(key, qv_val);
                                return Ok(QueryResult::with_stats(
                                    vec![crate::query::read::QueryRecord::Direct(QueryValue::Map(
                                        obj,
                                    ))],
                                    QueryStats {
                                        index_used: Some(format!(
                                            "sorted_idx_{}_min",
                                            def.name_interned
                                        )),
                                        records_scanned: 1,
                                        records_returned: 1,
                                        execution_time_us: start.elapsed().as_micros() as u64,
                                    },
                                )
                                .with_versions(collect_versions(
                                    query.with_version,
                                    self.mvcc_store_ref(),
                                    std::slice::from_ref(&id),
                                )));
                            }
                            Ok(None) => {
                                // Index genuinely has no entries — fall through to full scan.
                            }
                        }
                    }
                }
            }

            if let SelectItem::CountAll { alias } = &query.select.items[0] {
                let count: u64 = self.counter().get().await?;
                let key = alias.as_deref().unwrap_or("count").to_string();
                let mut obj = new_map_wc(1);
                obj.insert(key, QueryValue::Int(count as i64));
                return Ok(QueryResult::with_stats(
                    vec![crate::query::read::QueryRecord::Direct(QueryValue::Map(
                        obj,
                    ))],
                    QueryStats {
                        index_used: Some("__record_counter__".to_string()),
                        records_scanned: 0,
                        records_returned: 1,
                        execution_time_us: start.elapsed().as_micros() as u64,
                    },
                ));
            }
        }

        // ── V3.1: filtered ANN — And([VectorSimilarity, ...residual]) ──
        // Recognise the filtered-vector pattern BEFORE the generic index2
        // path so we can run ANN-with-oversample + post-filter. Falls
        // through to the legacy paths when the shape doesn't match.
        if let Some(ref filter) = query.r#where {
            if let Some(fvq) = try_extract_filtered_vector_query(filter) {
                return Box::pin(
                    self.read_filtered_vector_scan(query, ctx, interner, &fvq, tx, start),
                )
                .await;
            }
        }

        // ── index2: FTS / Functional / Vector accelerated path ─────
        //
        // CRIT-7: an `IndexResult::Set`/`Ranked` from index2 is the
        // AUTHORITATIVE, complete answer for this filter — including when
        // it is EMPTY. An empty set means "zero rows match", full stop.
        // We must NOT fall through to the base_index btree / full-scan paths
        // when `rids_vec` is empty:
        //   - FTS / functional: the full scan would re-derive the same
        //     empty answer at O(N) needless cost (tokenising every row
        //     again), plus mis-attribute `index_used`.
        //   - VectorSimilarity: the bare predicate compiles to
        //     `FilterNode::True` (it has no row-level representation), so
        //     a full-scan fall-through returns EVERY row in the table
        //     instead of zero — a correctness bug.
        if let Some(ref filter) = query.r#where {
            if let Some(candidates) = self.try_plan_index2(filter, interner).await? {
                let tag = if candidates.ranked {
                    "index2_ranked"
                } else {
                    "index2"
                };
                // Bare index2 planning is committed-only; keep materialisation
                // committed-only too. Tx-aware candidate planning is out of scope.
                let raw = self.get_many_bytes(&candidates.rids).await?;
                let rows = candidates
                    .rids
                    .into_iter()
                    .zip(raw)
                    .filter_map(|(id, bytes)| bytes.map(|b| (id, b)))
                    .collect();
                return Box::pin(self.postprocess_candidates(
                    query,
                    ctx,
                    interner,
                    rows,
                    tag,
                    start,
                    Vec::new(),
                ))
                .await;
            }
        }

        // Try index scan first (base_index btree)
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
                        let mut index_usable = true;
                        for values in &lookup_sets {
                            // P0-3a (#1011): `None` = a DROP of this index is
                            // in its drain→sweep window — do NOT count it as 0
                            // (a wrongly-empty count). Mark the index unusable
                            // and fall through to the full-scan tail, which
                            // recomputes the count correctly.
                            match self
                                .index_manager_ref()
                                .lookup_by_index(idx_name, values)
                                .await?
                            {
                                Some(ids) => total += ids.len() as u64,
                                None => {
                                    index_usable = false;
                                    break;
                                }
                            }
                        }
                        if !index_usable {
                            // Skip the count fast-path AND the `read_index_scan`
                            // call below (it would observe the same `None`): the
                            // DROP-window result must come from a full scan.
                        } else {
                            let key = alias.as_deref().unwrap_or("count").to_string();
                            let mut obj = new_map_wc(1);
                            obj.insert(key, QueryValue::Int(total as i64));
                            return Ok(QueryResult::with_stats(
                                vec![crate::query::read::QueryRecord::Direct(QueryValue::Map(
                                    obj,
                                ))],
                                QueryStats {
                                    index_used: Some(format!("idx_{idx_name}")),
                                    records_scanned: total,
                                    records_returned: 1,
                                    execution_time_us: start.elapsed().as_micros() as u64,
                                },
                            ));
                        }
                    }
                }

                // P0-3a (#1011): `read_index_scan` now returns
                // `Option<QueryResult>`: `None` means the index was momentarily
                // unusable (DROP in its drain→sweep window) — the index scan
                // returned no partial result, so fall through to the full-scan
                // tail below. A silent empty result here would be
                // indistinguishable from "the index has no matches".
                if let Some(result) = self
                    .read_index_scan(
                        query,
                        ctx,
                        interner,
                        idx_name,
                        &lookup_sets,
                        residual.as_ref(),
                        start,
                    )
                    .await?
                {
                    return Ok(result);
                }
                // Fall through to full scan.
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
                input: None,
                alias,
                ..
            } = &query.select.items[0]
            {
                if let Some(field_path) = intern_field_path(path, interner) {
                    // F-72 (#899, P0): state-filtered — see the MIN arm above.
                    // P0-3a (#1037): IndexDrainInProgress also falls through.
                    if let Some(def) = self.sorted_indexes().find_by_field_ready(&field_path) {
                        match self.sorted_indexes().lookup_max(def.name_interned).await {
                            Err(DbError::IndexDrainInProgress(_)) => {
                                // Index is in drain window — fall through to full scan.
                            }
                            Err(e) => return Err(e),
                            Ok(Some(id)) => {
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
                                    .unwrap_or_else(|| {
                                        path.last().map(|s| s.as_str()).unwrap_or("max")
                                    })
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
                                    versions: collect_versions(
                                        query.with_version,
                                        self.mvcc_store_ref(),
                                        std::slice::from_ref(&id),
                                    ),
                                    corrupt_records: Vec::new(),
                                    ..Default::default()
                                });
                            }
                            Ok(None) => {
                                // Index genuinely has no entries — fall through to full scan.
                            }
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
        // P0-3a (#1037): if the sorted index is being dropped, fall through to full scan.
        if let Some((idx_name, encoded_key, after_id, limit, direction)) =
            self.try_plan_keyset_seek_with_interner(query, interner, Some(ctx))
        {
            match self
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
                .await
            {
                Ok(result) => return Ok(result),
                Err(DbError::IndexDrainInProgress(_)) => {
                    // Index is in drain window — fall through to full scan below.
                }
                Err(e) => return Err(e),
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
        // P0-3a (#1037): if the sorted index is being dropped, fall through to full scan.
        if let Some((idx_name, take, skip, direction)) =
            self.try_plan_order_limit_fast_path_with_context(query, interner, Some(ctx))
        {
            match self
                .read_order_limit_fast(query, ctx, interner, idx_name, take, skip, direction, start)
                .await
            {
                Ok(result) => return Ok(result),
                Err(DbError::IndexDrainInProgress(_)) => {
                    // Index is in drain window — fall through to full scan below.
                }
                Err(e) => return Err(e),
            }
        }

        // Sorted-index plan (range / Gte / Lte / Between). Only kicks
        // in for the supported filter shapes — falls through to scan
        // otherwise.
        if let Some(ref filter) = query.r#where {
            if let Some((idx_name, lo, hi, residual)) =
                self.try_plan_sorted_index_scan(filter, interner)
            {
                // P0-3a (#1037): if the sorted index is being dropped, fall through
                // to full scan instead of returning an error. This preserves the
                // pre-fix behavior and matches slice 1's behavior for regular indexes.
                match self
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
                    .await
                {
                    Ok(result) => return Ok(result),
                    Err(DbError::IndexDrainInProgress(_)) => {
                        // Index is in drain window — fall through to full scan below.
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        // Range-from-AND extraction: try to pull a range predicate out
        // of an AND filter and use the sorted index for the range scan,
        // with remaining conjuncts as residual filter.
        if let Some(ref filter) = query.r#where {
            if let Some((idx_name, lo, hi, residual)) =
                self.try_plan_and_range_index_scan(filter, interner)
            {
                // P0-3a (#1037): same fall-through logic as the sorted-index plan above.
                match self
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
                    .await
                {
                    Ok(result) => return Ok(result),
                    Err(DbError::IndexDrainInProgress(_)) => {
                        // Index is in drain window — fall through to full scan below.
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        self.read_scan(query, ctx, interner, tx, encoding, batch_size, start)
            .await
    }
}
