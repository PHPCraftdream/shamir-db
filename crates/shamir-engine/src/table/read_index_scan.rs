//! Index-backed read execution for TableManager.
//!
//! Implements the sorted-index range scan (`read_sorted_index_scan`),
//! the ORDER BY + LIMIT K fast path (`read_order_limit_fast`), and the
//! equality/In index scan path (`read_index_scan`).

use std::time::Instant;

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

use super::read_exec::try_project_page_only;
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
                                });
                            }

                            let result_json = exec::apply_select(&matched, &query.select, interner);
                            let (paged_json, pagination) = exec::apply_pagination(
                                result_json,
                                &query.pagination,
                                query.count_total,
                            );
                            let paged: Vec<QueryRecord> =
                                paged_json.into_iter().map(QueryRecord::Json).collect();
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
        let records = self.get_many(&id_vec).await?;
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

        let mut result_json = if has_group_by {
            let group_by = query.group_by.as_ref().unwrap();
            exec::apply_group_by(&matched, group_by, &query.select, interner, ctx)
        } else if has_agg {
            exec::apply_aggregate_all(&matched, &query.select, interner)
        } else {
            exec::apply_select(&matched, &query.select, interner)
        };

        if let Some(ref order_by) = query.order_by {
            exec::apply_order_by(&mut result_json, order_by);
        }

        let (paged_json, pagination) =
            exec::apply_pagination(result_json, &query.pagination, query.count_total);
        let paged: Vec<QueryRecord> = paged_json.into_iter().map(QueryRecord::Json).collect();
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
        // Vectored fetch of the ids we actually need (post-skip,
        // pre-take). Stale ids → None; we collect Some until we hit
        // `take`. One trip to the data store, regardless of K.
        let needed: Vec<RecordId> = ids.into_iter().skip(skip).collect();
        let fetched = self.get_many(&needed).await?;
        let mut matched: Vec<(RecordId, InnerValue)> = Vec::with_capacity(take);
        for (id, opt) in needed.iter().zip(fetched) {
            if matched.len() == take {
                break;
            }
            if let Some(record) = opt {
                matched.push((*id, record));
            }
        }

        let result_json = exec::apply_select(&matched, &query.select, interner);
        let records_returned = result_json.len() as u64;
        let result: Vec<QueryRecord> = result_json.into_iter().map(QueryRecord::Json).collect();

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
            record_ids.extend(ids);
        }

        // 2. Compile residual filter if present
        let residual_cb: Option<FilterNode> = residual.map(|f| compile_filter(f, interner));

        // 3. Vectored fetch + per-record residual filter. Stale
        //    index entries materialise as None and are skipped.
        let id_vec: Vec<RecordId> = record_ids.iter().copied().collect();
        let records = self.get_many(&id_vec).await?;
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

        let mut result_json = if has_group_by {
            let group_by = query.group_by.as_ref().unwrap();
            exec::apply_group_by(&matched, group_by, &query.select, interner, ctx)
        } else if has_agg {
            exec::apply_aggregate_all(&matched, &query.select, interner)
        } else {
            exec::apply_select(&matched, &query.select, interner)
        };

        if query.select.distinct {
            result_json = exec::apply_distinct(result_json);
        }
        if let Some(ref order_by) = query.order_by {
            exec::apply_order_by(&mut result_json, order_by);
        }

        let (records_json, pagination) =
            exec::apply_pagination(result_json, &query.pagination, query.count_total);

        let elapsed = start.elapsed();
        let records_returned = records_json.len() as u64;
        let records: Vec<QueryRecord> = records_json.into_iter().map(QueryRecord::Json).collect();

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
}
