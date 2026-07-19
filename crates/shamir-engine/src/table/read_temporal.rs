//! Temporal read strategies for TableManager.
//!
//! Implements `read_as_of` (point-in-time / MVCC snapshot) and
//! `read_history` (per-record version timeline).

use std::time::Instant;

use futures::StreamExt;

use crate::query::filter::eval::{compile_filter, FilterNode};
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{
    exec, At, OrderDirection, QueryRecord, QueryResult, QueryStats, ReadQuery,
};
use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use super::read_exec::{apply_select_value_bytes, try_project_page_only_bytes};
use super::table_manager::TableManager;

impl TableManager {
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
    pub(super) async fn read_as_of(
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
        let stream = self.list_stream(FULL_SCAN_BATCH);
        futures::pin_mut!(stream);

        let mut matched: Vec<(RecordId, Bytes)> = Vec::new();
        let mut records_scanned: u64 = 0;

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            records_scanned += batch.len() as u64;
            for (id, _cow) in batch {
                // Read the AS-OF value — this is NOT the current value; it is
                // the value the record had at `version` (or None if it did not
                // exist yet / was already deleted at that point).
                let asof_bytes = mvcc.get_at(id.as_bytes(), version).await?;
                let Some(bytes) = asof_bytes else {
                    // Record did not exist at this version — exclude it.
                    continue;
                };
                // bytes is already Bytes from get_at.
                // Apply the WHERE filter to the AS-OF value (NOT the current
                // value). This ensures `AsOf` semantics: the filter evaluates
                // the world as it was at `version`.
                // S4: filter via RecordView lens (bare-scalar fallback to
                // InnerValue for non-map records).
                let passes = match filter_cb.as_ref() {
                    Some(cb) => match shamir_types::record_view::RecordView::new(&bytes) {
                        Ok(view) => cb.matches(&view, ctx),
                        Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                            Ok(iv) => cb.matches(&iv, ctx),
                            Err(_) => false,
                        },
                    },
                    None => true,
                };
                if passes {
                    matched.push((id, bytes));
                }
            }
        }

        // Pipeline tail — same helpers as the collecting / index-scan paths.
        // S4: matched is now Bytes-typed; all three branches consume Bytes.
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);

        if let Some((paged, pagination)) =
            try_project_page_only_bytes(query, &matched, interner, ctx.scalars.clone())
        {
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
                explain: None,
                skipped: false,
            });
        }

        let mut result_qv = if has_group_by {
            let group_by = query.group_by.as_ref().unwrap();
            exec::apply_group_by(&matched, group_by, &query.select, interner, ctx)
        } else if has_agg {
            exec::apply_aggregate_all(&matched, &query.select, interner, ctx.scalars.clone())
        } else {
            apply_select_value_bytes(&matched, &query.select, interner, ctx.scalars.clone())
        };

        if query.select.distinct {
            result_qv = exec::apply_distinct_qv(result_qv);
        }
        if let Some(ref order_by) = query.order_by {
            exec::apply_order_by_qv(&mut result_qv, order_by);
        }

        let (records_qv, pagination) =
            exec::apply_pagination(result_qv, &query.pagination, query.count_total);

        let records_returned = records_qv.len() as u64;
        let records: Vec<QueryRecord> = records_qv.into_iter().map(QueryRecord::Direct).collect();
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
            explain: None,
            skipped: false,
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
    pub(super) async fn read_history(
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

        let stream = self.list_stream(FULL_SCAN_BATCH);
        futures::pin_mut!(stream);
        let mut matched_ids: Vec<RecordId> = Vec::new();
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (id, cow) in batch {
                let passes = match (&filter_cb, &cow) {
                    (Some(cb), super::record_cow::RecordCow::Borrowed(b)) => {
                        match shamir_types::record_view::RecordView::new(b) {
                            Ok(view) => cb.matches(&view, ctx),
                            Err(_) => false,
                        }
                    }
                    (Some(cb), super::record_cow::RecordCow::Owned(record)) => {
                        cb.matches(record, ctx)
                    }
                    (None, _) => true,
                };
                if passes {
                    matched_ids.push(id);
                }
            }
        }

        // ── 2. For each matched id, pull its full timeline.
        let temporal = &query.temporal;
        let (from, to, limit, order) = match temporal {
            crate::query::read::Temporal::History {
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
            let timeline = mvcc.history_of(id.as_bytes()).await?;
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

        // ── 5. Project each version's value bytes via the RecordView lens
        //     and attach `_version` and `_ts`.
        let mut out_records: Vec<QueryRecord> = Vec::with_capacity(rows.len());
        for (id, version, ts, value_bytes) in rows {
            // Project directly from the version's storage bytes via the
            // RecordView lens (no full InnerValue decode for the common
            // map-record case; bare-scalar history values fall back to a
            // transient InnerValue inside the helper). A corrupt entry
            // (engine-written bytes — should never fire) yields a Null row
            // rather than being skipped.
            let mut projected = apply_select_value_bytes(
                &[(id, value_bytes)],
                &query.select,
                interner,
                ctx.scalars.clone(),
            );
            // apply_select_value returns one QueryValue per input record.
            let row_qv = projected
                .pop()
                .unwrap_or(QueryValue::Map(shamir_types::types::common::new_map_wc(0)));
            // Attach the timeline metadata. If projection produced a
            // Map, insert directly; otherwise wrap so the metadata has
            // a home.
            let final_qv = match row_qv {
                QueryValue::Map(mut map) => {
                    map.insert("_version".to_string(), QueryValue::Int(version as i64));
                    map.insert(
                        "_ts".to_string(),
                        match ts {
                            Some(t) => QueryValue::Int(t as i64),
                            None => QueryValue::Null,
                        },
                    );
                    QueryValue::Map(map)
                }
                other => {
                    let mut map = shamir_types::types::common::new_map_wc(3);
                    map.insert("value".to_string(), other);
                    map.insert("_version".to_string(), QueryValue::Int(version as i64));
                    map.insert(
                        "_ts".to_string(),
                        match ts {
                            Some(t) => QueryValue::Int(t as i64),
                            None => QueryValue::Null,
                        },
                    );
                    QueryValue::Map(map)
                }
            };
            out_records.push(QueryRecord::Direct(final_qv));
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
            explain: None,
            skipped: false,
        })
    }
}
