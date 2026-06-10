//! Temporal read strategies for TableManager.
//!
//! Implements `read_as_of` (point-in-time / MVCC snapshot) and
//! `read_history` (per-record version timeline).

use std::time::Instant;

use futures::StreamExt;

use crate::query::filter::eval::{compile_filter, FilterNode};
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{exec, At, OrderDirection, QueryResult, QueryStats, ReadQuery};
use shamir_storage::error::DbResult;
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use super::read_exec::try_project_page_only;
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
}
