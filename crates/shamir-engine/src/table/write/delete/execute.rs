use std::time::Instant;

use bytes::Bytes;
use futures::StreamExt;

use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::write::{DeleteOp, InsertedRecord, WriteResult};
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::record_view_to_query_value;
use shamir_types::record_view::RecordView;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::validator::WriteOp;
use shamir_types::access::Actor;

use crate::table::record_cow::RecordCow;
use crate::table::table_manager::TableManager;
use crate::table::write::set::execute::project_query_value;
use crate::table::write::write_helpers::validator_failure_to_db_error;
impl TableManager {
    /// tx-aware variant of DELETE.
    ///
    /// Filters records by `where_clause`, stages each removal via
    /// [`delete_tx`](Self::delete_tx). WAL is NOT opened here —
    /// `commit_tx` Phase 4 emits one V2 entry covering the whole tx.
    pub async fn execute_delete_tx(
        &self,
        op: &DeleteOp,
        ctx: &FilterContext<'_>,
        tx: &mut shamir_tx::TxContext,
        resolver: Option<&dyn crate::query::TableResolver>,
        actor: &Actor,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // Check whether any Delete validators are registered BEFORE the scan
        // so we know whether to carry bytes alongside ids.
        let has_delete_validators = {
            let bindings = self.validator_bindings();
            bindings.iter().any(|b| b.ops.contains(&WriteOp::Delete))
        };
        // RETURNING: when the caller opted in via `op.select`, the matched
        // records' raw bytes must be retained alongside the id so we can
        // de-intern them into a `QueryValue` map for the result. Combined
        // with `has_delete_validators` — either reason forces bytes-on.
        let wants_records = op.select.is_some();
        let keep_bytes = has_delete_validators || wants_records;

        // `to_delete`: (RecordId, Option<Bytes>).
        //
        // - Full-scan arm: the filter runs over a `RecordView` lens (no
        //   `InnerValue` tree decode). Bytes are kept alongside the id so the
        //   validator pass below can build a `RecordView` without a second
        //   store read, and so RETURNING can de-intern the row. When neither
        //   validators nor RETURNING are active, bytes are dropped (skip the
        //   per-row clone). `RecordCow::Owned` (aggregate / GROUP-BY path)
        //   has the tree already; we serialize it once if needed.
        // - Index arm: the planner already resolved the id set; bytes are
        //   serialised from the already-decoded `InnerValue` only when
        //   validators or RETURNING are active, otherwise the record is
        //   skipped.
        //
        // FG-3 (#729 residual, symmetric with execute_update_tx): both arms
        // below are committed-store-only, so a row this SAME tx already
        // staged (insert, or update to new bytes) that satisfies
        // `op.where_clause` never enters `to_delete`. After the scan we fold
        // in `staged_only_matches` — the tx's own staged `Set` rows not
        // already found above, re-evaluated against the SAME filter. The
        // actual delete-time semantics for a row already present (including
        // one this tx staged-removed already, which must NOT re-match) are
        // handled downstream by `delete_tx`'s existing RYOW-aware
        // `read_one_tx_bytes` read (a staged-removed id here would simply
        // no-op — `read_one_tx_bytes` returns `None` and `delete_tx` returns
        // `false`, contributing nothing to `affected`).
        let mut to_delete: Vec<(RecordId, Option<Bytes>)> =
            if let Some(via_index) = self.lookup_records_via_index(&op.where_clause, ctx).await? {
                via_index
                    .into_iter()
                    .map(|(id, iv)| {
                        let bytes = if keep_bytes { iv.to_bytes().ok() } else { None };
                        (id, bytes)
                    })
                    .collect()
            } else {
                let callback = compile_filter(&op.where_clause, interner);
                let mut result: Vec<(RecordId, Option<Bytes>)> = Vec::new();
                let stream = self.list_stream(batch_size);
                futures::pin_mut!(stream);
                while let Some(batch_result) = stream.next().await {
                    let batch = batch_result?;
                    for (id, cow) in batch {
                        match cow {
                            RecordCow::Borrowed(bytes) => {
                                // Zero-copy: try a lens over the raw bytes.
                                // Non-map records (bare scalars from legacy
                                // tests) fail RecordView::new — fall back to a
                                // full InnerValue decode for the filter match.
                                match RecordView::new(&bytes) {
                                    Ok(view) => {
                                        if callback.matches(&view, ctx) {
                                            result.push((id, Some(bytes)));
                                        }
                                    }
                                    Err(_) => {
                                        // Non-map: decode to InnerValue for filter.
                                        if let Ok(tree) = InnerValue::from_bytes(bytes.clone()) {
                                            if callback.matches(&tree, ctx) {
                                                result.push((id, Some(bytes)));
                                            }
                                        }
                                        // truly malformed → skip
                                    }
                                }
                            }
                            RecordCow::Owned(tree) => {
                                // Aggregate / GROUP-BY path — tree already in
                                // memory. Filter via the tree (implements
                                // RecordRef), serialise only when needed.
                                if callback.matches(&tree, ctx) {
                                    let bytes = if keep_bytes {
                                        tree.to_bytes().ok()
                                    } else {
                                        None
                                    };
                                    result.push((id, bytes));
                                }
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
        if tx.write_set.contains_key(&self.table_token()) {
            let already: shamir_collections::TSet<RecordId> =
                to_delete.iter().map(|(id, _)| *id).collect();
            // DeleteOp::where_clause is a mandatory Filter (never Option).
            let callback = compile_filter(&op.where_clause, interner);
            let keep = |_id: &RecordId, bytes: &Bytes| -> bool {
                match RecordView::new(bytes) {
                    Ok(view) => callback.matches(&view, ctx),
                    Err(_) => match InnerValue::from_bytes(bytes.clone()) {
                        Ok(tree) => callback.matches(&tree, ctx),
                        Err(_) => false,
                    },
                }
            };
            let staged_extra = crate::table::tx_scan_overlay::staged_only_matches(
                Some(tx),
                self.table_token(),
                &already,
                keep,
            );
            to_delete.extend(
                staged_extra
                    .into_iter()
                    .map(|(id, bytes)| (id, if keep_bytes { Some(bytes) } else { None })),
            );
        }

        // FG-2: optimistic-concurrency (CAS) gate — see execute_update_tx for
        // the full rationale. Same hybrid: immediate version_of check +
        // record_read_shared for SSI revalidation. Zero matches = no-op.
        if let Some(expected) = op.expected_version {
            let id_refs: Vec<RecordId> = to_delete.iter().map(|(id, _)| *id).collect();
            self.check_expected_version(&id_refs, expected, tx)?;
        }

        // S3: run validators on each record before deleting (tx).
        // Feeds a `RecordView` lens over the bytes collected above — no
        // second store read, no `InnerValue` tree decode.
        if has_delete_validators && !to_delete.is_empty() {
            for (_, maybe_bytes) in &to_delete {
                if let Some(bytes) = maybe_bytes {
                    let view = RecordView::new(bytes).map_err(|e| {
                        shamir_storage::error::DbError::Codec(format!(
                            "execute_delete_tx: malformed record bytes for validator: {}",
                            e
                        ))
                    })?;
                    // S3: run validators before staging (the W3c keystone pattern).
                    // The caller's `actor` is threaded so cross-table validator reads
                    // run with the caller's privileges (RI-7).
                    self.run_validators_view(
                        WriteOp::Delete,
                        None,
                        Some(&view),
                        actor,
                        tx,
                        resolver,
                    )
                    .await
                    .map_err(validator_failure_to_db_error)?;
                }
            }
        }

        // Stage the deletions and collect ids of actually-removed rows for
        // RETURNING. Only rows that `delete_tx` reports as removed (the id
        // existed in this tx's view) contribute to `affected` and to the
        // returned records.
        let mut affected: u64 = 0;
        let mut removed_with_bytes: Vec<Bytes> = Vec::new();
        for (id, maybe_bytes) in to_delete {
            if self.delete_tx(id, Some(&mut *tx)).await? {
                affected += 1;
                if wants_records {
                    if let Some(b) = maybe_bytes {
                        removed_with_bytes.push(b);
                    }
                }
            }
        }

        // Build RETURNING rows. Each removed record's bytes are de-interned
        // via the base interner (always safe — these are committed rows) and
        // wrapped as `InsertedRecord { id: None, fields }` so the wire shape
        // matches UPDATE-RETURNING. When `op.select.fields` is set, each row
        // is restricted to the named fields.
        let mut result_records: Vec<InsertedRecord> = Vec::new();
        if wants_records {
            let projection = op.select.as_ref().and_then(|s| s.fields.as_deref());
            result_records.reserve(removed_with_bytes.len());
            for bytes in &removed_with_bytes {
                let view = RecordView::new(bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "execute_delete_tx: RecordView for returning: {e}"
                    ))
                })?;
                let qv = record_view_to_query_value(&view, interner)?;
                let projected = project_query_value(qv, projection);
                result_records.push(InsertedRecord {
                    id: None,
                    fields: projected,
                });
            }
        }

        Ok(WriteResult {
            affected,
            records: result_records,
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }
}
