use bytes::Bytes;
use futures::StreamExt;

use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::BatchError;

use shamir_query_types::write::UpdateOp;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::query::batch::TableResolver;
use crate::query::filter::FilterContext;
use crate::query::TableRef;
use crate::table::TableManager;

use super::apply::field_is_nullable;
use super::helpers::cow_to_bytes;
use super::helpers::{
    action_name, classify_row_update, index_candidate_ids, qv_is_supported_scalar,
};
use super::scan::{
    any_child_references, collect_parent_values, discover_on_update_refs, resolve_field_id_layered,
};

/// A single reverse-FK reference whose `on_update != NoAction`.
pub(crate) struct OnUpdateRef {
    /// Child table name (same repo as the parent).
    pub(super) child_table: String,
    /// Child field path holding the FK value (first segment for the probe).
    pub(super) child_field: String,
    /// Parent field the child references (`ref_field`).
    pub(super) parent_ref_field: String,
    /// The `on_update` action (Restrict / Cascade / SetNull).
    pub(super) action: FkAction,
}

/// A single child mutation to apply inside the tx.
#[derive(Clone)]
pub(super) enum PendingMutation {
    /// Set this child row's FK field to a new scalar value (Cascade).
    UpdateField {
        table: TableManager,
        id: RecordId,
        field: String,
        new_value: QueryValue,
    },
    /// Set this child row's FK field to Null (SetNull).
    SetNull {
        table: TableManager,
        id: RecordId,
        field: String,
    },
}

/// A fully-resolved on-update fan-out plan — a flat list of child mutations
/// plus (for Restrict) an early-`Err` short-circuit.  Built at planning time
/// (before the tx); applied inside the tx via `apply_fk_update_plan`.
pub(crate) struct FkUpdatePlan {
    pub(super) mutations: Vec<PendingMutation>,
}

impl FkUpdatePlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.mutations.is_empty()
    }
}

/// Build the on-update fan-out plan for a parent `UPDATE`.
///
/// Performs, in order:
/// 1. **No-op gate** — collect `op.set` field keys; if none intersect a
///    discovered `on_update != NoAction` child `ref_field`, return empty plan.
/// 2. **Restrict** — if any Restrict-FK child references an old value being
///    changed, return `Err(fk_restrict)` immediately.
/// 3. **Cascade / SetNull** — collect child mutations for `old != new` pairs.
///
/// Returns `Ok(plan)` to proceed (possibly empty), or `Err` to reject.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn plan_fk_on_update(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
    parent_table: &TableManager,
    update_op: &UpdateOp,
    ctx: &FilterContext<'_>,
    alias: &str,
    tx: &shamir_tx::TxContext,
) -> Result<FkUpdatePlan, BatchError> {
    // 0. Extract the set-document field keys + scalar assignments.
    let set_fields: shamir_collections::TFxMap<String, QueryValue> = match &update_op.set {
        QueryValue::Map(m) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        // Non-map set (e.g. a scalar full-record replace) — no field-level
        // reassignment we can reason about → no fan-out.
        _ => {
            return Ok(FkUpdatePlan {
                mutations: Vec::new(),
            })
        }
    };
    if set_fields.is_empty() {
        return Ok(FkUpdatePlan {
            mutations: Vec::new(),
        });
    }

    // 1 + 2. Discover reverse-FK references with on_update != NoAction
    //    (routed through the repo's cached reverse-FK map, mirroring the
    //    delete path's `discover_action_refs` — #1181), with the cheap
    //    set-fields ∩ ref-fields intersection applied as the FIRST predicate
    //    inside the same filter pass, before the `on_update` check or the
    //    `OnUpdateRef` allocation (3 owned `String` clones): a no-op update
    //    (its `op.set` fields matching none of this parent's ref_fields)
    //    never pays for either. See `discover_on_update_refs`'s doc.
    let relevant_refs = discover_on_update_refs(resolver, parent_table_ref, &set_fields).await?;
    if relevant_refs.is_empty() {
        return Ok(FkUpdatePlan {
            mutations: Vec::new(),
        });
    }

    // 3. Validate the new_value is a supported literal scalar for each
    //    distinct relevant ref_field.  Computed / non-scalar → reject
    //    (least-surprise).
    //
    //    NOTE: we do NOT dedup `relevant_refs` itself — every `OnUpdateRef`
    //    must survive into the `by_table` grouping (step 5) so each individual
    //    FK reference gets its own cascade/setnull/restrict action, even when
    //    several references share the same parent field.  Only the derived
    //    field-name lists (`new_values` below, `ref_fields` in step 4) are
    //    de-duplicated — mirroring the delete path in `fk_actions.rs`, which
    //    dedups only a derived `parent_ref_fields` vector and keeps the full
    //    `action_refs` vector intact.  Previously this deduped `relevant_refs`
    //    by `parent_ref_field`, which silently collapsed distinct FK
    //    references sharing a parent field to one, so only one got its action
    //    applied (the rest — possibly a RESTRICT — were dropped entirely).
    let mut new_values: shamir_collections::TFxMap<String, QueryValue> =
        shamir_collections::TFxMap::default();
    for rref in &relevant_refs {
        if !new_values.contains_key(&rref.parent_ref_field) {
            let new = {
                #[expect(
                    clippy::expect_used,
                    reason = "guard: this field was filtered into relevant_refs from set_fields"
                )]
                set_fields
                    .get(&rref.parent_ref_field)
                    .expect("filtered above")
            };
            if !qv_is_supported_scalar(new) {
                return Err(BatchError::query_coded(
                    alias,
                    "fk_update_unsupported_new_value",
                    format!(
                        "ON UPDATE {}: cannot propagate non-scalar/computed new value \
                         for parent field '{}'",
                        action_name(rref.action),
                        rref.parent_ref_field,
                    ),
                ));
            }
            new_values.insert(rref.parent_ref_field.clone(), new.clone());
        }
    }

    // 4. Collect old parent ref_field values from rows matched by where.
    //    `where_clause = None` means update-all (no filter) — match every row.
    //    De-dup the field names only (one scan per distinct parent field);
    //    every reference survives for the `by_table` fan-out below.
    let ref_fields: Vec<&str> = {
        let mut fields: Vec<&str> = relevant_refs
            .iter()
            .map(|r| r.parent_ref_field.as_str())
            .collect();
        fields.sort_unstable();
        fields.dedup();
        fields
    };
    let old_parent_values = collect_parent_values(
        parent_table,
        update_op.where_clause.as_ref(),
        ctx,
        &ref_fields,
        tx,
    )
    .await
    .map_err(|e| BatchError::QueryError {
        alias: alias.to_string(),
        message: format!("fk_on_update: parent scan failed: {e}"),
        code: Some("fk_on_update".to_string()),
    })?;
    if old_parent_values.values().all(|v| v.is_empty()) {
        return Ok(FkUpdatePlan {
            mutations: Vec::new(),
        });
    }

    let mut mutations: Vec<PendingMutation> = Vec::new();

    // 5. For each relevant ref, walk children.
    // Group refs by child table (a child may have multiple FKs to the parent).
    let mut by_table: shamir_collections::TFxMap<String, Vec<&OnUpdateRef>> =
        shamir_collections::TFxMap::default();
    for rref in relevant_refs.iter() {
        by_table
            .entry(rref.child_table.clone())
            .or_default()
            .push(rref);
    }

    for (child_name, refs) in by_table {
        // Build the (old_value, new_value, action, child_field) probes for
        // this child.  Skip pairs where old == new (no actual change → no
        // fan-out for that row, and no restrict trigger).
        let mut probes: Vec<(&QueryValue, &QueryValue, FkAction, &str)> = Vec::new();
        // Grouped by child_field and deduped per field — see
        // `any_child_references`'s doc: this collapses what used to be one
        // full child-table scan PER (old-value, child_field) pair into one
        // scan per DISTINCT child_field, testing every row against the
        // whole set of old values for that field.
        let mut restrict_fields: shamir_collections::TFxMap<
            &str,
            shamir_collections::TFxSet<&QueryValue>,
        > = shamir_collections::TFxMap::default();
        for r in &refs {
            let new = match new_values.get(r.parent_ref_field.as_str()) {
                Some(v) => v,
                None => continue,
            };
            let old_vals = match old_parent_values.get(r.parent_ref_field.as_str()) {
                Some(v) if !v.is_empty() => v,
                _ => continue,
            };
            for old in old_vals {
                if old == new {
                    // No change for this old→new pair.
                    continue;
                }
                match r.action {
                    FkAction::Restrict => {
                        restrict_fields
                            .entry(r.child_field.as_str())
                            .or_default()
                            .insert(old);
                    }
                    FkAction::Cascade | FkAction::SetNull => {
                        probes.push((old, new, r.action, r.child_field.as_str()));
                    }
                    FkAction::NoAction => {}
                }
            }
        }

        // Resolve the child table.
        let child_table_ref = TableRef::with_repo(&parent_table_ref.repo, &child_name);
        let child_table =
            resolver
                .resolve(&child_table_ref)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!(
                        "fk_on_update: cannot resolve child table '{}': {e}",
                        child_name
                    ),
                    code: Some("fk_on_update".to_string()),
                })?;

        // 5a. RESTRICT — one pass per distinct child_field, testing every
        //     child row against the whole deduped old-value set for that
        //     field; reject immediately on the first match (no partial
        //     application).
        if !restrict_fields.is_empty() {
            for (&child_field, old_values) in &restrict_fields {
                let values: Vec<&QueryValue> = old_values.iter().copied().collect();
                let exists = any_child_references(&child_table, child_field, &values, tx)
                    .await
                    .map_err(|e| BatchError::QueryError {
                        alias: alias.to_string(),
                        message: format!("fk_on_update: restrict child scan failed: {e}"),
                        code: Some("fk_on_update".to_string()),
                    })?;
                if exists {
                    return Err(BatchError::query_coded(
                        alias,
                        "fk_restrict",
                        format!(
                            "cannot update '{}': value still referenced by '{}.{}'",
                            parent_table_ref.table, child_name, child_field,
                        ),
                    ));
                }
            }
        }

        if probes.is_empty() {
            continue;
        }

        // 5b. CASCADE / SET NULL — scan child rows, enqueue mutations.
        let interner = child_table
            .interner()
            .get()
            .await
            .map_err(|e| BatchError::QueryError {
                alias: alias.to_string(),
                message: format!("fk_on_update: child interner: {e}"),
                code: Some("fk_on_update".to_string()),
            })?;

        // For SetNull, validate the child field is nullable up front
        // (fail closed before staging any mutation).
        for (_, _, _, field) in probes.iter().filter(|p| p.2 == FkAction::SetNull) {
            if !field_is_nullable(&child_table, field).await {
                return Err(BatchError::query_coded(
                    alias,
                    "set_null_requires_nullable",
                    format!(
                        "SET NULL FK on '{}.{}' requires a nullable field",
                        child_table.name(),
                        field
                    ),
                ));
            }
        }

        // F-28 Step 2: resolve each probe's field id through the tx-layered
        // interner — see `resolve_field_id_layered`'s doc. A child row
        // inserted/updated earlier in this SAME tx may use a field name
        // never touched outside this tx (overlay-only until commit); a
        // base-only lookup would silently fail to match it.
        let mut resolved_probes: Vec<(&QueryValue, &QueryValue, FkAction, &str, u64)> =
            Vec::with_capacity(probes.len());
        for (old, new, action, field) in &probes {
            if let Some(field_id) = resolve_field_id_layered(interner, tx, field).await {
                resolved_probes.push((old, new, *action, field, field_id));
            }
        }
        // F-28 Step 5 (memo §2.4): do NOT `continue` on an empty
        // `resolved_probes` — `list_stream_tx` below records the
        // Serializable `TableScan` predicate at construction time, before
        // being polled. Skipping the scan when every probed field failed to
        // resolve (e.g. a brand-new/empty child table) would silently skip
        // recording that predicate too. The per-row loop below is a no-op
        // when `resolved_probes` is empty, so running the scan anyway is
        // cheap insurance.

        // F-40b (RI barrier): record this child-table scan at ENTRY,
        // regardless of isolation — same recording as
        // `fk_restrict.rs::any_child_references`, closing the
        // cross-transaction race for the explicit-Snapshot parent UPDATE
        // path (CASCADE / SET NULL fan-out).
        //
        // Recorded BEFORE the index fast-path branch below so it fires
        // regardless of which sub-path executes — the F-40b mutual-
        // serialization guarantee holds whether the index fast-path
        // short-circuits the scan or the `list_stream_tx` fallback runs.
        tx.record_ri_barrier(child_table.table_token());

        // F-53c: index fast-path (mirrors `any_child_references`'s index
        // lookup for RESTRICT, and `fk_actions::index_candidate_ids` for the
        // delete path). When every probe's FK field resolves to a BASE
        // interner id with a single-field index AND the current tx has no
        // staged writes against this child table, fetch candidate RecordIds
        // directly via `lookup_by_index` instead of streaming + matching
        // every row. The "no staged writes" gate is the correctness
        // boundary: the index reflects committed state only, so staged
        // inserts/updates/deletes (invisible to the index) must be handled
        // by the tx-aware scan fallback below.
        let index_probes: Vec<(&str, &QueryValue)> = resolved_probes
            .iter()
            .map(|(old, _, _, field, _)| (*field, *old))
            .collect();
        let no_staged_child_writes = tx
            .write_set
            .get(&child_table.table_token())
            .is_none_or(|s| s.is_empty());
        let fast_path = if no_staged_child_writes {
            index_candidate_ids(&child_table, interner, &index_probes)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!("fk_on_update: child index lookup failed: {e}"),
                    code: Some("fk_on_update".to_string()),
                })?
        } else {
            None
        };

        if let Some(candidate_ids) = fast_path {
            // Index-verified candidate set (committed-only; no staged writes
            // to reconcile). Re-read each candidate and run the SAME
            // per-field probe-matching inner loop as the scan fallback
            // (`classify_row_update`) so the fan-out is identical to the scan
            // path — guaranteeing the same rows are affected whether or not
            // an index exists.
            for id in candidate_ids {
                let bytes = match child_table.read_one_tx_bytes(id, Some(tx)).await {
                    Ok(Some(b)) => b,
                    Ok(None) => continue,
                    Err(e) => {
                        return Err(BatchError::QueryError {
                            alias: alias.to_string(),
                            message: format!(
                                "fk_on_update: child index fast-path re-read failed: {e}"
                            ),
                            code: Some("fk_on_update".to_string()),
                        })
                    }
                };
                classify_row_update(
                    id,
                    bytes.as_ref(),
                    &resolved_probes,
                    &child_table,
                    &mut mutations,
                );
            }
        } else {
            // Fallback: full scan with field match. Tx-aware via
            // `list_stream_tx` (overlays this tx's own staged writes so a
            // staged insert/update/delete on the child table is correctly
            // reflected). `list_stream_tx` also records the Serializable
            // `TableScan` predicate at construction time — the RI-barrier
            // token recorded at ENTRY above is the path-agnostic safety net
            // (exactly as in `any_child_references`).
            let batch_size = 1000;
            let stream = child_table.list_stream_tx(Some(tx), batch_size);
            futures::pin_mut!(stream);
            while let Some(batch_result) = stream.next().await {
                let batch = batch_result.map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!("fk_on_update: child scan failed: {e}"),
                    code: Some("fk_on_update".to_string()),
                })?;
                for (id, cow) in batch {
                    let bytes: Bytes = cow_to_bytes(cow).map_err(|e| BatchError::QueryError {
                        alias: alias.to_string(),
                        message: format!("fk_on_update: child scan serialize: {e}"),
                        code: Some("fk_on_update".to_string()),
                    })?;
                    classify_row_update(
                        id,
                        bytes.as_ref(),
                        &resolved_probes,
                        &child_table,
                        &mut mutations,
                    );
                }
            }
        }
    }

    Ok(FkUpdatePlan { mutations })
}
