//! Index-backed read execution for TableManager.
//!
//! Implements the sorted-index range scan (`read_sorted_index_scan`),
//! the ORDER BY + LIMIT K fast path (`read_order_limit_fast`), and the
//! equality/In index scan path (`read_index_scan`).

use std::time::Instant;

use bytes::Bytes;

use crate::index::sorted_index_manager::decode_covering_projection;
use crate::query::filter::eval::{compile_filter, FilterNode};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::Filter;
use crate::query::read::{exec, QueryRecord, QueryResult, QueryStats, ReadQuery, SelectItem};
use shamir_storage::error::DbResult;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use super::read_exec::{
    apply_select_value_bytes, try_project_page_only, try_project_page_only_bytes,
};
use super::table_manager::TableManager;

impl TableManager {
    /// Scan a sorted index for a range of record_ids, then apply the
    /// usual read pipeline (residual filter, projection, group_by,
    /// aggregates, sort, paginate).
    #[allow(clippy::too_many_arguments)] // read-path parameters mirror query plan fields
    pub(super) async fn read_sorted_index_scan(
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
                                            .live_version(id.as_bytes())
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
                                                .into_key();
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
                                let recs = self.get_many(&fallback).await?;
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
                                    explain: None,
                                });
                            }

                            let result_qv =
                                exec::apply_select_value(&matched, &query.select, interner);
                            let (paged_qv, pagination) = exec::apply_pagination(
                                result_qv,
                                &query.pagination,
                                query.count_total,
                            );
                            let paged: Vec<QueryRecord> =
                                paged_qv.into_iter().map(QueryRecord::Direct).collect();
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
                                explain: None,
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

        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);
        let needs_inner = has_group_by || has_agg;

        let id_vec: Vec<RecordId> = record_ids.iter().copied().collect();

        // S4: aggregate paths now use bytes + RecordView lens (same as the
        // plain SELECT branch). No full InnerValue decode per row.
        if needs_inner {
            // ── Aggregate / GROUP BY branch (S4 — zero-copy RecordView lens) ──
            let raw = self.get_many_bytes(&id_vec).await?;
            let mut matched: Vec<(RecordId, Bytes)> = Vec::with_capacity(id_vec.len());
            for (id, opt) in id_vec.iter().zip(raw) {
                if let Some(bytes) = opt {
                    let passes = match &residual_cb {
                        Some(cb) => {
                            match shamir_types::record_view::RecordView::new(&bytes) {
                                Ok(view) => cb.matches(&view, ctx),
                                Err(_) => {
                                    // Bare-scalar fallback: decode to InnerValue.
                                    match InnerValue::from_bytes(bytes.as_ref()) {
                                        Ok(iv) => cb.matches(&iv, ctx),
                                        Err(_) => false,
                                    }
                                }
                            }
                        }
                        None => true,
                    };
                    if passes {
                        matched.push((*id, bytes));
                    }
                }
            }

            let records_scanned = matched.len() as u64;

            // try_project_page_only gates out for aggregates/group_by (always
            // None here) — skip the call entirely on this branch.

            let mut result_qv = if has_group_by {
                let group_by = query.group_by.as_ref().unwrap();
                exec::apply_group_by(&matched, group_by, &query.select, interner, ctx)
            } else {
                exec::apply_aggregate_all(&matched, &query.select, interner)
            };

            if let Some(ref order_by) = query.order_by {
                exec::apply_order_by_qv(&mut result_qv, order_by);
            }

            let (paged_qv, pagination) =
                exec::apply_pagination(result_qv, &query.pagination, query.count_total);
            let paged: Vec<QueryRecord> = paged_qv.into_iter().map(QueryRecord::Direct).collect();
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
                explain: None,
            })
        } else {
            // ── Plain SELECT branch (S3 — zero-copy RecordView lens) ─────────
            let raw = self.get_many_bytes(&id_vec).await?;
            let mut matched: Vec<(RecordId, Bytes)> = Vec::with_capacity(id_vec.len());
            for (id, opt) in id_vec.iter().zip(raw) {
                if let Some(bytes) = opt {
                    let passes = match &residual_cb {
                        Some(cb) => {
                            match shamir_types::record_view::RecordView::new(&bytes) {
                                Ok(view) => cb.matches(&view, ctx),
                                Err(_) => {
                                    // Bare-scalar fallback: decode to InnerValue.
                                    match InnerValue::from_bytes(bytes.as_ref()) {
                                        Ok(iv) => cb.matches(&iv, ctx),
                                        Err(_) => false,
                                    }
                                }
                            }
                        }
                        None => true,
                    };
                    if passes {
                        matched.push((*id, bytes));
                    }
                }
            }

            let records_scanned = matched.len() as u64;

            // Opt #3a (LIMIT push-down)
            if let Some((paged, pagination)) =
                try_project_page_only_bytes(query, &matched, interner)
            {
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
                    explain: None,
                });
            }

            let mut result_qv = apply_select_value_bytes(&matched, &query.select, interner);

            if let Some(ref order_by) = query.order_by {
                exec::apply_order_by_qv(&mut result_qv, order_by);
            }

            let (paged_qv, pagination) =
                exec::apply_pagination(result_qv, &query.pagination, query.count_total);
            let paged: Vec<QueryRecord> = paged_qv.into_iter().map(QueryRecord::Direct).collect();
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
                explain: None,
            })
        }
    }

    /// Execute the ORDER BY LIMIT K fast path: pull `skip + take`
    /// record ids from the sorted index in the requested direction,
    /// skip the offset, load + project.
    #[allow(clippy::too_many_arguments)] // read-path parameters mirror query plan fields
    pub(super) async fn read_order_limit_fast(
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
        // S3: zero-copy bytes path — no aggregate in this fast path.
        let needed: Vec<RecordId> = ids.into_iter().skip(skip).collect();
        let fetched = self.get_many_bytes(&needed).await?;
        let mut matched: Vec<(RecordId, Bytes)> = Vec::with_capacity(take);
        for (id, opt) in needed.iter().zip(fetched) {
            if matched.len() == take {
                break;
            }
            if let Some(bytes) = opt {
                matched.push((*id, bytes));
            }
        }

        let result_qv = apply_select_value_bytes(&matched, &query.select, interner);
        let records_returned = result_qv.len() as u64;
        let result: Vec<QueryRecord> = result_qv.into_iter().map(QueryRecord::Direct).collect();

        Ok(QueryResult {
            records: result,
            stats: Some(QueryStats {
                index_used: Some(label),
                records_scanned,
                records_returned,
                execution_time_us: start.elapsed().as_micros() as u64,
            }),
            // #128 sibling regression: this sorted-index LIMIT fast path
            // dropped pagination the same way the top-K heap did. Route it
            // through the shared helper so the wire contract holds.
            pagination: exec::fast_path_pagination(&query.pagination),
            value: None,
            explain: None,
        })
    }

    /// Execute the keyset-seek (Pagination::After) fast path.
    ///
    /// Uses the sorted index to fetch all record IDs in the half-plane
    /// beyond the seek key (inclusive boundary from `lookup_range`),
    /// then excludes rows whose ORDER BY value equals the seek value
    /// (exclusive semantics), sorts by ORDER BY direction, and takes
    /// `limit`.
    ///
    /// ASC  → lower bound = seek key, upper = open; strictly greater.
    /// DESC → lower = open, upper bound = seek key; strictly less.
    #[allow(clippy::too_many_arguments)] // read-path parameters mirror query plan fields
    pub(super) async fn read_keyset_seek(
        &self,
        query: &ReadQuery,
        _ctx: &FilterContext<'_>,
        interner: &Interner,
        index_name: u64,
        encoded_key: &[u8],
        limit: usize,
        direction: shamir_query_types::read::OrderDirection,
        start: Instant,
    ) -> DbResult<QueryResult> {
        use shamir_query_types::read::OrderDirection;

        // Audit 1.2: walk the sorted index in value order, dropping the
        // boundary rows (value == seek) inline and stopping after `limit`
        // survivors — O(limit + |rows == seek|) per page instead of the
        // old O(remaining table) fetch → project → full-sort → truncate.
        // The index returns IDs already in ORDER BY direction, so no
        // post-fetch sort is needed.
        let forward = matches!(direction, OrderDirection::Asc);
        let id_vec = self
            .sorted_indexes()
            .lookup_range_first_k(index_name, encoded_key, limit, forward)
            .await?;

        // Fetch record bytes (already value-ordered, already ≤ limit).
        let raw = self.get_many_bytes(&id_vec).await?;

        let mut matched: Vec<(RecordId, Bytes)> = Vec::with_capacity(id_vec.len());
        for (id, opt) in id_vec.iter().zip(raw) {
            if let Some(bytes) = opt {
                matched.push((*id, bytes));
            }
        }

        // Project to QueryValue in value order — no sort, no exclusion
        // filter (both are now handled by the ordered early-stop walk).
        let result_qv = apply_select_value_bytes(&matched, &query.select, interner);

        let records_scanned = matched.len() as u64;
        let records_returned = result_qv.len() as u64;
        let result: Vec<QueryRecord> = result_qv.into_iter().map(QueryRecord::Direct).collect();

        Ok(QueryResult {
            records: result,
            stats: Some(QueryStats {
                index_used: Some(format!("sorted_idx_{index_name}_keyset")),
                records_scanned,
                records_returned,
                execution_time_us: start.elapsed().as_micros() as u64,
            }),
            pagination: exec::fast_path_pagination(&query.pagination),
            value: None,
            explain: None,
        })
    }
    ///
    /// Index scan path: fetch records by index, apply residual filter + pipeline.
    ///
    /// `lookup_sets` contains one or more value sets to look up.
    /// For Eq — one set. For In — one set per value. Results are unioned.
    #[allow(clippy::too_many_arguments)] // read-path parameters mirror query plan fields
    pub(super) async fn read_index_scan(
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
            // Audit 1.5/3.2: `ids` is now `Arc<[RecordId]>` — a sorted
            // slice (O(1) cache-hit). Iterate the contiguous buffer to
            // union into the result set.
            record_ids.extend(ids.iter().copied());
        }

        // 2. Compile residual filter if present
        let residual_cb: Option<FilterNode> = residual.map(|f| compile_filter(f, interner));

        let id_vec: Vec<RecordId> = record_ids.iter().copied().collect();

        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);
        let needs_inner = has_group_by || has_agg;

        // Resolve index name for stats — needed by both paths below.
        let index_name_str = interner
            .get_str(&InternerKey::new(index_name))
            .map(|arc| arc.to_string())
            .unwrap_or_else(|| index_name.to_string());

        // S4: aggregate paths now use bytes + RecordView lens (same as the
        // plain SELECT branch). No full InnerValue decode per row.
        if needs_inner {
            // ── Aggregate / GROUP BY branch (S4 — zero-copy RecordView lens) ──
            let raw = self.get_many_bytes(&id_vec).await?;
            let mut matched: Vec<(RecordId, Bytes)> = Vec::with_capacity(id_vec.len());
            for (id, opt) in id_vec.iter().zip(raw) {
                if let Some(bytes) = opt {
                    let passes = match &residual_cb {
                        Some(cb) => {
                            match shamir_types::record_view::RecordView::new(&bytes) {
                                Ok(view) => cb.matches(&view, ctx),
                                Err(_) => {
                                    // Bare-scalar fallback: decode to InnerValue.
                                    match InnerValue::from_bytes(bytes.as_ref()) {
                                        Ok(iv) => cb.matches(&iv, ctx),
                                        Err(_) => false,
                                    }
                                }
                            }
                        }
                        None => true,
                    };
                    if passes {
                        matched.push((*id, bytes));
                    }
                }
            }

            let records_scanned = matched.len() as u64;

            // try_project_page_only gates out for aggregates/group_by (always
            // None here) — skip the call entirely on this branch.

            let mut result_qv = if has_group_by {
                let group_by = query.group_by.as_ref().unwrap();
                exec::apply_group_by(&matched, group_by, &query.select, interner, ctx)
            } else {
                exec::apply_aggregate_all(&matched, &query.select, interner)
            };

            if query.select.distinct {
                result_qv = exec::apply_distinct_qv(result_qv);
            }
            if let Some(ref order_by) = query.order_by {
                exec::apply_order_by_qv(&mut result_qv, order_by);
            }

            let (records_qv, pagination) =
                exec::apply_pagination(result_qv, &query.pagination, query.count_total);

            let elapsed = start.elapsed();
            let records_returned = records_qv.len() as u64;
            let records: Vec<QueryRecord> =
                records_qv.into_iter().map(QueryRecord::Direct).collect();

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
                explain: None,
            })
        } else {
            // ── Plain SELECT branch (S3 — zero-copy RecordView lens) ─────────
            let raw = self.get_many_bytes(&id_vec).await?;
            let mut matched: Vec<(RecordId, Bytes)> = Vec::with_capacity(id_vec.len());
            for (id, opt) in id_vec.iter().zip(raw) {
                if let Some(bytes) = opt {
                    let passes = match &residual_cb {
                        Some(cb) => {
                            match shamir_types::record_view::RecordView::new(&bytes) {
                                Ok(view) => cb.matches(&view, ctx),
                                Err(_) => {
                                    // Bare-scalar fallback: decode to InnerValue.
                                    match InnerValue::from_bytes(bytes.as_ref()) {
                                        Ok(iv) => cb.matches(&iv, ctx),
                                        Err(_) => false,
                                    }
                                }
                            }
                        }
                        None => true,
                    };
                    if passes {
                        matched.push((*id, bytes));
                    }
                }
            }

            let records_scanned = matched.len() as u64;

            // Opt #3a (LIMIT push-down)
            if let Some((paged, pagination)) =
                try_project_page_only_bytes(query, &matched, interner)
            {
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
                    explain: None,
                });
            }

            let mut result_qv = apply_select_value_bytes(&matched, &query.select, interner);

            if query.select.distinct {
                result_qv = exec::apply_distinct_qv(result_qv);
            }
            if let Some(ref order_by) = query.order_by {
                exec::apply_order_by_qv(&mut result_qv, order_by);
            }

            let (records_qv, pagination) =
                exec::apply_pagination(result_qv, &query.pagination, query.count_total);

            let elapsed = start.elapsed();
            let records_returned = records_qv.len() as u64;
            let records: Vec<QueryRecord> =
                records_qv.into_iter().map(QueryRecord::Direct).collect();

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
                explain: None,
            })
        }
    }
}
