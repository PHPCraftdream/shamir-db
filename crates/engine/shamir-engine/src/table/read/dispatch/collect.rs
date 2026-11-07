use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;

use crate::query::filter::eval::FilterNode;
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{exec, CorruptRecordRef, QueryRecord, QueryResult, QueryStats, ReadQuery};
use crate::table::support::storage::record_cow::RecordCow;
use crate::table::table_manager::TableManager;
use bytes::Bytes;

use shamir_storage::error::DbResult;
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;

use crate::table::read::execution::dispatch::{collect_versions, DynBatchStream};

impl TableManager {
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
        // Use bytes-level pre-filter when a compiled filter is present: rows
        // that definitely don't match are skipped before full InnerValue decode.
        // Both arms are boxed to unify the two opaque `impl Stream` types.
        // When a tx context is present, use tx-aware list_stream_tx so SSI
        // read-set recording is fused into this single scan. The bytes-level
        // pre-filter is skipped in that case — SSI correctness requires
        // recording every candidate row (not just pre-filter survivors), and
        // the compiled filter_cb in the loop below is the authoritative gate.

        let stream: DynBatchStream<'_> = if tx.is_some() {
            Box::pin(self.list_stream_tx_after_predicate(tx, batch_size))
        } else if let Some(pf) = pre_filter {
            Box::pin(self.list_stream_filtered(batch_size, pf))
        } else {
            Box::pin(self.list_stream(batch_size))
        };

        Box::pin(self.read_collecting_from(query, ctx, interner, filter_cb, stream, start)).await
    }

    pub(in crate::table::read) async fn read_collecting_from(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        filter_cb: Option<&FilterNode>,
        mut stream: DynBatchStream<'_>,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);
        let needs_raw = has_group_by || has_agg;

        // F-7 (#797): the with_version + (GROUP BY | aggregate | DISTINCT)
        // combination is rejected up-front in `read_impl` before any dispatch,
        // so by the time we reach here with `with_version == true` we are
        // guaranteed to be on the plain-ORDER BY path (no row collapse) — the
        // id-threading logic below is correct under that invariant.

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

        let mut records_scanned: u64 = 0;

        // F-53a (#874): decide the inline top-K bounded-heap path UP FRONT,
        // before the scan. When the query has ORDER BY + a finite LIMIT and no
        // DISTINCT / GROUP BY / aggregate, we feed each WHERE-passing row
        // directly into a `k = skip + take` bounded max-heap DURING the scan —
        // never accumulating the full O(N) `rec_acc` the pre-F-53a shape
        // materialised before `apply_order_by_topk` ever trimmed it. The old
        // code comment claiming "O(K) memory" was true only for the heap's
        // own internals; the `rec_acc` Vec feeding it was still O(N).
        //
        // `count_total` and `with_version` NO LONGER force the fallback (the
        // pre-F-53a gate excluded both): `count_total` is satisfied by an
        // independent running counter over WHERE-passing rows, and
        // `with_version` is satisfied by carrying each heap item's `RecordId`.
        // DISTINCT / GROUP BY / aggregates still force the fallback (they need
        // the full projected/byte materialisation).
        let (skip_resolved, take_resolved) = query.pagination.resolve();
        let use_topk = query.order_by.is_some()
            && take_resolved.is_some()
            && !query.select.distinct
            && !has_group_by
            && !has_agg;

        // Two accumulation modes: raw Bytes (for aggregates) or projected
        // QueryRecord (for plain SELECT). S4: the aggregate arm now carries
        // Bytes + per-row RecordView instead of a full InnerValue tree.
        let mut raw_acc: Vec<(RecordId, Bytes)> = Vec::new();
        let mut rec_acc: Vec<crate::query::read::QueryRecord> = Vec::new();
        // F-7 (#797): plain ORDER BY + with_version — track each surviving
        // row's RecordId alongside rec_acc so the per-record versions array
        // can be rebuilt after the sort reorders rows. Only allocated when the
        // client asked for versions AND this table is MVCC-backed (mirrors the
        // tracking_versions pattern in read_counting and the index2 path).
        let tracking_versions = query.with_version && self.mvcc_store_ref().is_some();
        let mut id_acc: Vec<RecordId> = if tracking_versions {
            Vec::with_capacity(16)
        } else {
            Vec::new()
        };
        let proj = if !needs_raw {
            Some(exec::SelectProjection::new_with_context(
                &query.select,
                interner,
                ctx.scalars.clone(),
                Some(ctx),
            )?)
        } else {
            None
        };

        // F-53a (#874): the inline bounded heap. Only constructed when
        // `use_topk`; the scan loop pushes into it instead of `rec_acc`.
        // `with_ids` mirrors `tracking_versions` so the post-heap version
        // rebuild (`collect_versions`) still gets the surviving page's ids.
        let mut topk_heap: Option<exec::TopKHeap<'_>> = if use_topk {
            #[expect(
                clippy::unwrap_used,
                reason = "guard: use_topk proves order_by and finite take are present"
            )]
            let order_by = query.order_by.as_ref().unwrap();
            #[expect(
                clippy::unwrap_used,
                reason = "guard: use_topk proves finite take is present"
            )]
            let take = take_resolved.unwrap() as usize;
            Some(exec::TopKHeap::new(
                order_by,
                skip_resolved as usize,
                take,
                tracking_versions,
            ))
        } else {
            None
        };
        // Independent running counter for `count_total` — decoupled from any
        // Vec's `.len()`: increments once per WHERE-passing row regardless of
        // whether it survives the heap's eviction. Lets the bounded heap serve
        // `count_total` queries without falling back to the O(N) full sort.
        let mut topk_total: u64 = 0;

        // F-10 (#800): rows that fail to decode are still skipped from the
        // result set (unchanged behaviour) but reported here instead of
        // silently vanishing.
        let mut corrupt: Vec<CorruptRecordRef> = Vec::new();

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            records_scanned += batch.len() as u64;
            for (id, cow) in batch {
                match cow {
                    RecordCow::Borrowed(b) => {
                        let view = match shamir_types::record_view::RecordView::new(&b) {
                            Ok(v) => v,
                            Err(_) => {
                                // malformed row → skip, but report it.
                                corrupt.push(CorruptRecordRef {
                                    table: self.name().to_string(),
                                    id,
                                });
                                continue;
                            }
                        };
                        let passes = match filter_cb {
                            Some(cb) => cb.matches(&view, ctx),
                            None => true,
                        };
                        if passes {
                            if let Some(heap) = topk_heap.as_mut() {
                                // F-53a (#874): inline bounded heap — project
                                // and push straight in, never accumulating the
                                // full rec_acc. `topk_total` is the running
                                // `count_total` counter (independent of the
                                // heap's K-row window).
                                topk_total += 1;
                                let Some(projection) = proj.as_ref() else {
                                    return Err(shamir_storage::error::DbError::Validation(
                                        "top-K projection is unavailable".to_string(),
                                    ));
                                };
                                let qv = projection.project_value(&view, interner)?;
                                if let Some(meter) = ctx.row_byte_meter {
                                    meter.charge(
                                        crate::query::batch::exec_scope::RowByteMeter::value_size(
                                            &qv,
                                        ),
                                    )?;
                                }
                                heap.push(qv, tracking_versions.then_some(id));
                            } else if needs_raw {
                                // S4: push raw bytes; the aggregate pipeline
                                // builds a RecordView per row. No InnerValue
                                // decode here.
                                // Meter the retained raw intermediate: 16-byte
                                // list entry + 16-byte id + 24-byte byte slot
                                // + encoded length. It grants no output credit.
                                if let Some(meter) = ctx.row_byte_meter {
                                    meter.charge(56u64.saturating_add(b.len() as u64))?;
                                }
                                raw_acc.push((id, b));
                            } else {
                                #[expect(
                                    clippy::unwrap_used,
                                    reason = "guard: !needs_raw initializes the projection before rows are scanned"
                                )]
                                let row = QueryRecord::Direct(
                                    proj.as_ref().unwrap().project_value(&view, interner)?,
                                );
                                if let Some(meter) = ctx.row_byte_meter {
                                    meter.charge_record(&row)?;
                                }
                                rec_acc.push(row);
                                if tracking_versions {
                                    id_acc.push(id);
                                }
                            }
                        }
                    }
                    RecordCow::Owned(record) => {
                        let passes = match filter_cb {
                            Some(cb) => cb.matches(&record, ctx),
                            None => true,
                        };
                        if passes {
                            if let Some(heap) = topk_heap.as_mut() {
                                // F-53a (#874): inline bounded heap (Owned arm
                                // — already-decoded InnerValue, e.g. from MVCC).
                                topk_total += 1;
                                let Some(projection) = proj.as_ref() else {
                                    return Err(shamir_storage::error::DbError::Validation(
                                        "top-K projection is unavailable".to_string(),
                                    ));
                                };
                                let qv = projection.project_value(&record, interner)?;
                                if let Some(meter) = ctx.row_byte_meter {
                                    meter.charge(
                                        crate::query::batch::exec_scope::RowByteMeter::value_size(
                                            &qv,
                                        ),
                                    )?;
                                }
                                heap.push(qv, tracking_versions.then_some(id));
                            } else if needs_raw {
                                // S4: the Owned arm carries an already-decoded
                                // InnerValue (e.g. from MVCC). Re-encode to
                                // bytes once so the aggregate lens can consume
                                // it. This is the cold path (the hot Borrowed
                                // arm avoids the re-encode).
                                match record.to_bytes() {
                                    Ok(bytes) => {
                                        if let Some(meter) = ctx.row_byte_meter {
                                            meter
                                                .charge(56u64.saturating_add(bytes.len() as u64))?;
                                        }
                                        raw_acc.push((id, bytes));
                                    }
                                    // Malformed tree — skip defensively, but report it.
                                    Err(_) => {
                                        corrupt.push(CorruptRecordRef {
                                            table: self.name().to_string(),
                                            id,
                                        });
                                        continue;
                                    }
                                }
                            } else {
                                #[expect(
                                    clippy::unwrap_used,
                                    reason = "guard: !needs_raw initializes the projection before rows are scanned"
                                )]
                                let row = QueryRecord::Direct(
                                    proj.as_ref().unwrap().project_value(&record, interner)?,
                                );
                                if let Some(meter) = ctx.row_byte_meter {
                                    meter.charge_record(&row)?;
                                }
                                rec_acc.push(row);
                                if tracking_versions {
                                    id_acc.push(id);
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut paged_ids: Vec<RecordId> = Vec::new();
        let (paged, pagination) = if let Some(heap) = topk_heap {
            // F-53a (#874): the inline bounded heap already applied the
            // ORDER BY + skip/take window DURING the scan — drain, sort, and
            // slice here. No full `rec_acc` / `qv_result` was built for this
            // path, so `apply_order_by_topk` / `apply_order_by_qv` are never
            // reached. `topk_total` (the independent running counter) feeds
            // `count_total` metadata without falling back to the O(N) sort.
            //
            // `with_version` composes: when `tracking_versions`, each heap item
            // carried its `RecordId`, and `into_sorted` returns the surviving
            // page's ids already aligned with the values — drop them into
            // `paged_ids` so the existing `collect_versions` call below
            // rebuilds the per-record versions array unchanged.
            let (qvs, ids) = heap.into_sorted();
            if tracking_versions {
                paged_ids = ids;
            }
            (
                qvs,
                exec::topk_pagination(&query.pagination, query.count_total, topk_total),
            )
        } else {
            // Fallback path (no ORDER BY + finite LIMIT, OR DISTINCT / GROUP BY
            // / aggregate present): unchanged — build qv_result from the full
            // accumulation, then sort + paginate.
            let mut qv_result: Vec<shamir_types::types::value::QueryValue> = if has_group_by {
                exec::validate_aggregate_select(&query.select)?;
                #[expect(
                    clippy::unwrap_used,
                    reason = "guard: has_group_by selects this branch only when group_by is present"
                )]
                let group_by = query.group_by.as_ref().unwrap();
                exec::try_apply_group_by(&raw_acc, group_by, &query.select, interner, ctx)?
            } else if has_agg {
                exec::validate_aggregate_select(&query.select)?;
                crate::query::read::aggregate::apply_aggregate_all_with_context(
                    &raw_acc,
                    &query.select,
                    interner,
                    ctx,
                )?
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
            if let Some(ref order_by) = query.order_by {
                if tracking_versions {
                    // F-7 (#797): plain ORDER BY + with_version — sort the
                    // records AND the companion id_acc with the SAME
                    // permutation so the per-record versions array stays
                    // aligned with records after the reorder.
                    exec::apply_order_by_qv_with_ids(&mut qv_result, &mut id_acc, order_by);
                } else {
                    exec::apply_order_by_qv(&mut qv_result, order_by);
                }
            }
            if tracking_versions {
                // Zip records+ids so a single `apply_pagination` slices BOTH
                // in lockstep (skip/take), keeping versions[i] aligned with
                // records[i] after pagination. Unzip into the page and the
                // paged id slice used below to build `versions`.
                let paired: Vec<(shamir_types::types::value::QueryValue, RecordId)> =
                    qv_result.into_iter().zip(id_acc).collect();
                let (sliced, pg) =
                    exec::apply_pagination(paired, &query.pagination, query.count_total);
                let mut recs = Vec::with_capacity(sliced.len());
                for (r, i) in sliced {
                    recs.push(r);
                    paged_ids.push(i);
                }
                (recs, pg)
            } else {
                exec::apply_pagination(qv_result, &query.pagination, query.count_total)
            }
        };
        let final_records: Vec<crate::query::read::QueryRecord> = paged
            .into_iter()
            .map(crate::query::read::QueryRecord::Direct)
            .collect();

        if !needs_raw {
            if let Some(meter) = ctx.row_byte_meter {
                meter.credit_survivors(&final_records);
            }
        }
        let elapsed = start.elapsed();
        let records_returned = final_records.len() as u64;
        let records = final_records;

        // F-7 (#797): build the per-record versions array from the FINAL,
        // paginated id slice (paged_ids), not the pre-pagination full list —
        // only the surviving page's ids map to the surviving page's records.
        // collect_versions returns None when with_version is false or the
        // table has no MVCC backing (mirrors the existing FG-2 pattern).
        let versions = collect_versions(query.with_version, self.mvcc_store_ref(), &paged_ids);

        Ok(QueryResult::with_stats(
            records,
            QueryStats {
                index_used: None,
                records_scanned,
                records_returned,
                execution_time_us: elapsed.as_micros() as u64,
            },
        )
        .with_pagination(pagination)
        .with_versions(versions)
        .with_corrupt_records(corrupt))
    }
}
