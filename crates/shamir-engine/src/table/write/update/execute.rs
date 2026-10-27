use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::StreamExt;

use crate::function::builtin_scalars;
use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::write::{InsertedRecord, UpdateOp, UpdateReturnMode, WriteResult};
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::{
    merge_storage_bytes, query_value_to_inner_with, record_view_to_query_value,
};
use shamir_types::record_view::RecordView;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue, Value};

use crate::validator::WriteOp;
use shamir_types::access::Actor;

use crate::table::record_cow::RecordCow;
use crate::table::table_manager::TableManager;
use crate::table::write::set::execute::project_query_value;
use crate::table::write::write_helpers::{
    apply_transforms, intern_via_layered, make_layered_interner, resolve_computed_record,
    validator_failure_to_db_error,
};
impl TableManager {
    /// tx-aware variant of UPDATE.
    ///
    /// Filters records by `where_clause`, merges `set` fields, then
    /// stages each changed record via [`update_tx`](Self::update_tx).
    /// WAL is NOT opened here — `commit_tx` Phase 4 emits one V2
    /// entry covering the whole tx. Returns the same `WriteResult`
    /// shape. Interner / counter persistence is **skipped** —
    /// commit_tx handles that uniformly.
    pub async fn execute_update_tx(
        &self,
        op: &UpdateOp,
        ctx: &FilterContext<'_>,
        tx: &mut shamir_tx::TxContext,
        resolver: Option<&dyn crate::query::TableResolver>,
        actor: &Actor,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        // Table name is immutable for the whole call — hash it ONCE and
        // reuse the `u64` everywhere below, instead of `self.table_token()`
        // (a fresh SipHash-style hash over the name on every call) re-hashing
        // per matched row in the loop further down.
        let table_token = self.table_token();
        let interner = self.interner().get().await?;

        // Resolve inline `$fn` computed fields, then apply server-side
        // transforms (③.2d — AutoNow stamps updated_at on every write),
        // then intern field names through the tx overlay.
        let mut resolved_set = resolve_computed_record(&op.set, interner)
            .map_err(shamir_storage::error::DbError::Codec)?;
        // ③.2d UPDATE-path: apply transforms with is_insert=false.
        // Only AutoNow runs here (injects updated_at into the set-map);
        // AutoNowAdd/ComputedDefault are gated on is_insert and are skipped.
        // Fast-skip when the table declares no transforms (common hot path).
        let transforms = self.schema_transforms();
        if !transforms.is_empty() {
            let now_ns: u64 = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            apply_transforms(
                resolved_set.to_mut(),
                &transforms,
                builtin_scalars(),
                now_ns,
                false,
            );
        }
        let set_inner = {
            let layered = make_layered_interner(interner, tx);
            let intern_fn = intern_via_layered(&layered);
            query_value_to_inner_with(&resolved_set, &intern_fn)
                .map_err(shamir_storage::error::DbError::from)?
        };
        let set_map = match &set_inner {
            InnerValue::Map(m) => m,
            _ => {
                return Err(shamir_storage::error::DbError::Validation(
                    "UPDATE set must produce a Map".to_string(),
                ))
            }
        };
        // #1205: `set_map` is the ONLY new content every matched row's
        // `merge_storage_bytes` call below patches onto its own old bytes —
        // capture its referenced ids ONCE here (already an in-memory tree,
        // no extra decode) instead of `pre_commit_prelock`'s A8 scan
        // re-decoding every merged row's full bytes. The old-bytes portion
        // of each merge needs no capture: those ids were already covered by
        // A8 at THEIR OWN original staging time.
        shamir_tx::collect_referenced_ids(&set_inner, &mut tx.referenced_interner_ids);

        // Collect matched records as raw storage bytes — no InnerValue tree
        // decode on the hot scan path. The index-via-index arm returns
        // pre-decoded InnerValue trees; serialize them once to bytes so the
        // downstream merge/change-detect/index-plan pipeline is uniform.
        //
        // FG-3 (#729 residual): the committed-store scan below (both the
        // index-path and list_stream-fallback arms) is blind to this tx's
        // own `write_set` — a row this SAME tx already staged (inserted, or
        // updated to new bytes) that satisfies `filter`/`op.where_clause`
        // would otherwise never enter `matched`. After each arm runs, we
        // append `staged_only_matches` — the tx's staged `Set` rows not
        // already found by the arm above, re-evaluated against the SAME
        // filter (or unconditionally kept when there is no WHERE). Rows
        // ALREADY in `matched` are left untouched here — the per-row loop
        // below already resolves the correct "effective old bytes" from
        // `tx.write_set` for those (see `effective_old_bytes`).
        let mut matched: Vec<(RecordId, Bytes)> = if let Some(ref filter) = op.where_clause {
            if let Some(via_index) = self.lookup_records_via_index(filter, ctx).await? {
                via_index
                    .into_iter()
                    .map(|(id, iv)| {
                        let b = iv.to_bytes().map_err(|e| {
                            shamir_storage::error::DbError::Codec(format!(
                                "execute_update_tx: index-path serialize: {e}"
                            ))
                        })?;
                        Ok((id, b))
                    })
                    .collect::<DbResult<Vec<_>>>()?
            } else {
                let callback = compile_filter(filter, interner);
                let mut result = Vec::new();
                let stream = self.list_stream(batch_size);
                futures::pin_mut!(stream);
                while let Some(batch_result) = stream.next().await {
                    let batch = batch_result?;
                    for (id, cow) in batch {
                        match cow {
                            RecordCow::Borrowed(bytes) => {
                                // Zero-copy lens for the filter; keep raw bytes.
                                match RecordView::new(&bytes) {
                                    Ok(view) => {
                                        if callback.matches(&view, ctx) {
                                            result.push((id, bytes));
                                        }
                                    }
                                    Err(_) => {
                                        // Non-map: decode to InnerValue for filter.
                                        if let Ok(tree) = InnerValue::from_bytes(bytes.clone()) {
                                            if callback.matches(&tree, ctx) {
                                                result.push((id, bytes));
                                            }
                                        }
                                    }
                                }
                            }
                            RecordCow::Owned(tree) => {
                                if callback.matches(&tree, ctx) {
                                    let bytes = tree.to_bytes().map_err(|e| {
                                        shamir_storage::error::DbError::Codec(format!(
                                            "execute_update_tx: owned serialize: {e}"
                                        ))
                                    })?;
                                    result.push((id, bytes));
                                }
                            }
                        }
                    }
                }
                result
            }
        } else {
            let mut result = Vec::new();
            let stream = self.list_stream(batch_size);
            futures::pin_mut!(stream);
            while let Some(batch_result) = stream.next().await {
                for (id, cow) in batch_result? {
                    match cow {
                        RecordCow::Borrowed(bytes) => result.push((id, bytes)),
                        RecordCow::Owned(tree) => {
                            let bytes = tree.to_bytes().map_err(|e| {
                                shamir_storage::error::DbError::Codec(format!(
                                    "execute_update_tx: owned serialize: {e}"
                                ))
                            })?;
                            result.push((id, bytes));
                        }
                    }
                }
            }
            result
        };

        // FG-3: fold in purely-staged-inserted rows this tx's own write_set
        // holds that the committed-store scan above could never see. Gated
        // on the tx having ANY staged rows for this table BEFORE compiling
        // the filter — a tx that never wrote this table pays nothing beyond
        // the `write_set.get(token)` probe.
        if tx.write_set.contains_key(&table_token) {
            let already: shamir_collections::TSet<RecordId> =
                matched.iter().map(|(id, _)| *id).collect();
            let residual_callback = op
                .where_clause
                .as_ref()
                .map(|f| compile_filter(f, interner));
            let keep = |_id: &RecordId, bytes: &Bytes| -> bool {
                match &residual_callback {
                    None => true,
                    Some(callback) => match RecordView::new(bytes) {
                        Ok(view) => callback.matches(&view, ctx),
                        Err(_) => match InnerValue::from_bytes(bytes.clone()) {
                            Ok(tree) => callback.matches(&tree, ctx),
                            Err(_) => false,
                        },
                    },
                }
            };
            let staged_extra = crate::table::tx_scan_overlay::staged_only_matches(
                Some(tx),
                table_token,
                &already,
                keep,
            );
            matched.extend(staged_extra);
        }

        let mut affected: u64 = 0;
        let mut result_records: Vec<InsertedRecord> = Vec::with_capacity(matched.len());
        let return_mode = op
            .select
            .as_ref()
            .map(|s| s.return_mode)
            .unwrap_or(UpdateReturnMode::Changed);
        let wants_records = op.select.is_some();

        // FG-2: optimistic-concurrency (CAS) gate — `expected_version`.
        //
        // Hybrid check (brief §"Validation timing"): for every matched row,
        // (1) immediately compare `MvccStore::version_of(key)` against
        // `expected_version` — a mismatch aborts the WHOLE op with
        // `VersionConflict` BEFORE any row is staged; AND (2) register each
        // matched key as a tx read at `expected_version` via
        // `record_read_shared`, so the EXISTING SSI `validate_read_set` at
        // commit time closes the race window between this check and commit.
        //
        // Zero matched rows is a no-op (affected: 0) — matching the existing
        // convention for a WHERE that matches nothing.
        if let Some(expected) = op.expected_version {
            let id_refs: Vec<RecordId> = matched.iter().map(|(id, _)| *id).collect();
            self.check_expected_version(&id_refs, expected, tx)?;
        }

        // Check whether any Update validators are registered BEFORE the
        // per-row loop, mirroring `execute_delete_tx`'s `has_delete_validators`.
        // When false, the per-row `old_qv` / `new_qv` de-intern (String alloc
        // per key + owned values) + deep `.clone()` is pure waste —
        // `run_validators_qv` returns instantly when zero validators are bound
        // for `WriteOp::Update` (audit `2026-07-06-perf-hot-paths.md` §1.3).
        let has_update_validators = {
            let bindings = self.validator_bindings();
            bindings.iter().any(|b| b.ops.contains(&WriteOp::Update))
        };

        for (id, old_bytes) in &matched {
            // Read-this-tx-staging-FIRST: a prior op in the SAME batch/tx
            // (e.g. an FK `ON UPDATE CASCADE`/`SET NULL` fan-out applied by
            // `apply_fk_update_plan` BEFORE this op) may have already staged
            // a DIFFERENT value for this exact row. The `matched` scan above
            // is COMMITTED-store-only and is blind to this tx's own
            // `write_set` (see `table_manager_streaming.rs`'s "KNOWN
            // LIMITATION" doc on `list_stream_tx`). Merging on top of the
            // stale committed snapshot and then staging via `update_tx_bytes`
            // (which calls `StagingStore::set` — an unconditional replace by
            // design) would silently OVERWRITE the earlier staged mutation.
            //
            // So we resolve the effective "old" bytes from this tx's own
            // staging first and merge on top of THAT — mirroring the
            // read-your-own-write probe `read_one_tx_bytes` does for point
            // reads. This keeps BOTH the byte-level merge AND the downstream
            // index-delta planning in `update_tx_bytes` correctly incremental
            // (cascade-result → final, composing with the earlier call's
            // committed → cascade-result delta, instead of double-counting).
            let effective_old_bytes: Bytes = match tx
                .write_set
                .get(&table_token)
                .and_then(|staging| staging.staged_op(id.as_bytes()))
            {
                Some(shamir_tx::staging_store::StagedKind::Set(staged_bytes)) => {
                    // This tx already staged a value here (read-your-own-
                    // writes): merge on top of the staged value, not the
                    // committed snapshot, so the earlier mutation survives.
                    staged_bytes
                }
                Some(shamir_tx::staging_store::StagedKind::Removed) => {
                    // This tx already staged a DELETE for this row (e.g. a
                    // same-batch DELETE op in this transactional batch). From
                    // this tx's point of view the row no longer exists, so
                    // this UPDATE must NOT match it — skip the row entirely
                    // (no merge, no stage, no affected-count contribution).
                    // Without this skip, `update_tx_bytes` would stage a Set
                    // over the staged Remove, silently resurrecting the
                    // deleted row and losing the DELETE intent.
                    continue;
                }
                None => old_bytes.clone(),
            };

            // Byte-level merge: patch effective_old_bytes with the interned
            // set overlay. Produces bytes identical to
            // merge_inner_maps(old_tree, set_map).to_bytes().
            let new_bytes = merge_storage_bytes(&effective_old_bytes, set_map)?;

            // Change detection: a no-op set yields identical bytes (the
            // merge copies old value spans verbatim when no key is patched),
            // so byte equality is safe for this "patched from old" shape.
            let changed = new_bytes.as_ref() != effective_old_bytes.as_ref();

            // Track the validator-built `old_qv` so the RETURNING block below
            // can REUSE it instead of de-interning `old_bytes` a second time.
            // Only populated when validators actually ran (which implies
            // `has_update_validators && changed`); `None` otherwise.
            let mut reuse_old_qv: Option<QueryValue> = None;

            if changed {
                if has_update_validators {
                    // S3: run validators before staging (tx overlay-aware).
                    //
                    // Build old/new QueryValues WITHOUT decoding an InnerValue tree:
                    // - old_qv: de-intern the committed bytes via RecordView (base
                    //   interner — all keys are committed).
                    // - new_qv: overlay the string-keyed resolved_set on top of
                    //   old_qv (no overlay-minted interned ids to resolve — the
                    //   overlay keys are already strings in resolved_set). This
                    //   matches the W3a result-QueryValue pattern and avoids the
                    //   "Interned key not found" failure that would occur if we
                    //   de-interned new_bytes through the base-only reverse
                    //   snapshot (new_bytes may contain overlay-minted key ids
                    //   not yet in base).
                    let old_view = RecordView::new(&effective_old_bytes).map_err(|e| {
                        shamir_storage::error::DbError::Codec(format!(
                            "execute_update_tx: RecordView for validator old: {e}"
                        ))
                    })?;
                    let old_qv = record_view_to_query_value(&old_view, interner)?;
                    let new_qv = {
                        let mut m = match old_qv.clone() {
                            Value::Map(m) => m,
                            _ => shamir_types::types::common::new_map(),
                        };
                        if let Value::Map(overlay) = resolved_set.as_ref() {
                            for (k, v) in overlay {
                                m.insert(k.clone(), v.clone());
                            }
                        }
                        QueryValue::Map(m)
                    };
                    self.run_validators_qv(
                        WriteOp::Update,
                        Some(&new_qv),
                        Some(&old_qv),
                        actor,
                        Some(tx),
                        resolver,
                    )
                    .await
                    .map_err(validator_failure_to_db_error)?;

                    // Make the validator-built old_qv available for RETURNING
                    // reuse (avoid re-de-interning the same bytes below).
                    reuse_old_qv = Some(old_qv);
                }

                // #1205: `new_bytes`'s new content is `set_map`, whose ids
                // were captured once above, before this row loop.
                self.update_tx_bytes(*id, &effective_old_bytes, new_bytes.clone(), &mut *tx, true)
                    .await?;
                affected += 1;
            }

            if wants_records {
                let should_include = match return_mode {
                    UpdateReturnMode::All => true,
                    UpdateReturnMode::Changed => changed,
                    UpdateReturnMode::Unchanged => !changed,
                };
                if should_include {
                    // Build the result QueryValue from the old record (base-interned
                    // keys — always safe) overlaid with the resolved SET fields
                    // (string-keyed QueryValue — no overlay ids). Decoding
                    // new_bytes via the base interner would fail when op.set
                    // introduces brand-new field names that were only interned
                    // into the tx overlay and are not yet in base.
                    //
                    // REUSE the validator-built `old_qv` when validators ran
                    // (avoid a second `RecordView::new` + de-intern of the SAME
                    // `old_bytes`). Only de-intern here when the validator path
                    // was skipped (no Update validators bound).
                    let base_qv = match reuse_old_qv.take() {
                        Some(qv) => qv,
                        None => {
                            let old_view = RecordView::new(&effective_old_bytes).map_err(|e| {
                                shamir_storage::error::DbError::Codec(format!(
                                    "execute_update_tx: RecordView for result: {e}"
                                ))
                            })?;
                            record_view_to_query_value(&old_view, interner)?
                        }
                    };
                    let mut m = match base_qv {
                        Value::Map(m) => m,
                        _ => shamir_types::types::common::new_map(),
                    };
                    if let Value::Map(overlay) = resolved_set.as_ref() {
                        for (k, v) in overlay {
                            m.insert(k.clone(), v.clone());
                        }
                    }
                    // Optional field projection (UpdateSelect.fields). When
                    // present, restrict the returned row to the named fields
                    // — symmetric with INSERT/DELETE RETURNING projections.
                    let projection = op.select.as_ref().and_then(|s| s.fields.as_deref());
                    let fields = project_query_value(QueryValue::Map(m), projection);
                    result_records.push(InsertedRecord { id: None, fields });
                }
            }
        }

        Ok(WriteResult {
            affected,
            records: result_records,
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }
}
