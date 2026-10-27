use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;

use crate::query::filter::eval::FilterNode;
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{
    exec, CorruptRecordRef, PaginationInfo, QueryRecord, QueryResult, QueryStats, ReadQuery,
};
use crate::table::support::storage::record_cow::RecordCow;
use crate::table::table_manager::TableManager;

use serde_bytes::ByteBuf;
use shamir_query_types::batch::ResultEncoding;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::codecs::interned::record_view_to_id_msgpack;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::record_id::RecordId;

use crate::table::read::execution::dispatch::{
    collect_versions, intern_simple_projection_ids, is_select_all, is_select_simple, DynBatchStream,
};

impl TableManager {
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

        let proj = exec::SelectProjection::new(&query.select, interner, ctx.scalars.clone())?
            .with_context(ctx);

        // When a tx context is present, use tx-aware list_stream_tx so SSI
        // read-set recording is fused into this single scan (see read_collecting).
        let mut stream: DynBatchStream<'_> = if tx.is_some() {
            Box::pin(self.list_stream_tx_after_predicate(tx, batch_size))
        } else if let Some(pf) = pre_filter {
            Box::pin(self.list_stream_filtered(batch_size, pf))
        } else {
            Box::pin(self.list_stream(batch_size))
        };

        let mut records_scanned: u64 = 0;
        let mut matched_total: u64 = 0;
        let mut result: Vec<crate::query::read::QueryRecord> = Vec::new();
        // FG-2: track which RecordId survived filter+pagination so we can
        // emit a parallel `versions` array when `with_version == true`.
        // Only populated when an MvccStore backs this table.
        let tracking_versions = query.with_version && self.mvcc_store_ref().is_some();
        let mut version_ids: Vec<RecordId> = if tracking_versions {
            Vec::with_capacity(16)
        } else {
            Vec::new()
        };
        // F-10 (#800): rows that fail to decode are still skipped from the
        // result set (unchanged behaviour) but reported here instead of
        // silently vanishing.
        let mut corrupt: Vec<CorruptRecordRef> = Vec::new();

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            records_scanned += batch.len() as u64;

            for (id, cow) in &batch {
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
                                    let row =
                                        QueryRecord::Direct(proj.project_value($rec, interner)?);
                                    if let Some(meter) = ctx.row_byte_meter {
                                        meter.charge_record(&row)?;
                                    }
                                    if tracking_versions {
                                        version_ids.push(*id);
                                    }
                                    result.push(row);
                                }
                            } else {
                                let row = QueryRecord::Direct(proj.project_value($rec, interner)?);
                                if let Some(meter) = ctx.row_byte_meter {
                                    meter.charge_record(&row)?;
                                }
                                if tracking_versions {
                                    version_ids.push(*id);
                                }
                                result.push(row);
                            }
                        }
                    }};
                }

                match cow {
                    RecordCow::Borrowed(b) => {
                        let view = match shamir_types::record_view::RecordView::new(b) {
                            Ok(v) => v,
                            Err(_) => {
                                // malformed row → skip, but report it.
                                corrupt.push(CorruptRecordRef {
                                    table: self.name().to_string(),
                                    id: *id,
                                });
                                continue;
                            }
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

        Ok(QueryResult::with_stats(
            result,
            QueryStats {
                index_used: None,
                records_scanned,
                records_returned,
                execution_time_us: elapsed.as_micros() as u64,
            },
        )
        .with_pagination(pagination)
        .with_versions(collect_versions(
            query.with_version,
            self.mvcc_store_ref(),
            &version_ids,
        ))
        .with_corrupt_records(corrupt))
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

        let proj = exec::SelectProjection::new(&query.select, interner, ctx.scalars.clone())?
            .with_context(ctx);

        // When a tx context is present, use tx-aware list_stream_tx so SSI
        // read-set recording is fused into this single scan (see read_collecting).
        let mut stream: DynBatchStream<'_> = if tx.is_some() {
            Box::pin(self.list_stream_tx_after_predicate(tx, batch_size))
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
        // FG-2: track per-result RecordId for `with_version` reporting.
        let tracking_versions = query.with_version && self.mvcc_store_ref().is_some();
        let mut version_ids: Vec<RecordId> = if tracking_versions {
            Vec::with_capacity(16)
        } else {
            Vec::new()
        };
        // F-10 (#800): rows that fail to decode are still skipped from the
        // result set (unchanged behaviour) but reported here instead of
        // silently vanishing.
        let mut corrupt: Vec<CorruptRecordRef> = Vec::new();

        while let Some(batch_result) = stream.next().await {
            if done {
                break;
            }
            let batch = batch_result?;
            records_scanned += batch.len() as u64;

            for (id, cow) in &batch {
                // ── Id-keyed pass-through path (S-read) ─────────────────────
                if use_id_encoding {
                    match cow {
                        RecordCow::Borrowed(b) => {
                            let view = match shamir_types::record_view::RecordView::new(b) {
                                Ok(v) => v,
                                Err(_) => {
                                    // malformed row → skip, but report it.
                                    corrupt.push(CorruptRecordRef {
                                        table: self.name().to_string(),
                                        id: *id,
                                    });
                                    continue;
                                }
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
                                        QueryRecord::Direct(proj.project_value(&view, interner)?)
                                    }
                                }
                            };
                            if let Some(meter) = ctx.row_byte_meter {
                                meter.charge_record(&record)?;
                            }
                            if tracking_versions {
                                version_ids.push(*id);
                            }
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
                                                proj.project_value(iv, interner)?,
                                            ),
                                        }
                                    }
                                    Err(_) => {
                                        QueryRecord::Direct(proj.project_value(iv, interner)?)
                                    }
                                }
                            };
                            if let Some(meter) = ctx.row_byte_meter {
                                meter.charge_record(&record)?;
                            }
                            if tracking_versions {
                                version_ids.push(*id);
                            }
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
                        let row = QueryRecord::Direct(proj.project_value($rec, interner)?);
                        if let Some(meter) = ctx.row_byte_meter {
                            meter.charge_record(&row)?;
                        }
                        if tracking_versions {
                            version_ids.push(*id);
                        }
                        result.push(row);
                    }};
                }

                match cow {
                    RecordCow::Borrowed(b) => {
                        let view = match shamir_types::record_view::RecordView::new(b) {
                            Ok(v) => v,
                            Err(_) => {
                                // malformed row → skip, but report it.
                                corrupt.push(CorruptRecordRef {
                                    table: self.name().to_string(),
                                    id: *id,
                                });
                                continue;
                            }
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

        Ok(QueryResult::with_stats(
            result,
            QueryStats {
                index_used: None,
                records_scanned,
                records_returned,
                execution_time_us: elapsed.as_micros() as u64,
            },
        )
        .with_pagination(pagination)
        .with_versions(collect_versions(
            query.with_version,
            self.mvcc_store_ref(),
            &version_ids,
        ))
        .with_corrupt_records(corrupt))
    }
}
