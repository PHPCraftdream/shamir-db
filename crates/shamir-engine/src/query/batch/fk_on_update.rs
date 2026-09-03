//! Phase ②.2b — `ON UPDATE` referential-action enforcement.
//!
//! Mirrors the Phase D delete-path (`fk_restrict` + `fk_actions`) but fires on
//! the **UPDATE** path: when an `UPDATE` reassigns a parent *referenced*
//! `ref_field` to a new value, dependent child rows are fanned out according
//! to their FK's `on_update` action.
//!
//! ## Trigger condition (the fast no-op gate)
//!
//! Unlike delete (where *any* matched row removal is a trigger), an update
//! only matters if `op.set` actually reassigns one of the parent
//! `ref_field`s that a child FK with `on_update != NoAction` points at.  The
//! vast majority of updates touch ordinary fields, so [`plan_fk_on_update`]
//! **first** collects the set of field keys from `op.set` and intersects them
//! with discovered child `parent_ref_field`s; if the intersection is empty it
//! returns an empty plan immediately — zero scan overhead on the hot path.
//!
//! ## (old → new) pairs
//!
//! For each intersected `ref_field`, `new_value` is the literal scalar that
//! `op.set` assigns to that field.  `old_values` are the current
//! `ref_field` values from the parent rows matched by `op.where_clause`
//! (collected via the shared `collect_parent_values` scan).  Fan-out only
//! happens for `old != new` pairs.
//!
//! ## MVP scope — single level, literal scalar `new`
//!
//! - **Depth:** one level.  A re-keyed child row is *not* itself re-scanned
//!   for grandchildren.  This mirrors the conservative single-field MVP of
//!   Phase D and avoids FK-cycle recursion entirely (a re-key can otherwise
//!   ping-pong forever on a self/mutual FK cycle).  Multi-level update-cascade
//!   is tracked as a follow-up.
//! - **`new_value`:** only a literal scalar (`Int`/`Str`/`Bool`/`F64`/`Null`/
//!   `Bin`/`Dec`/`Big`) is propagated.  If `op.set` assigns the `ref_field` a
//!   computed (`$fn`/`$ref`/`$expr`) or non-scalar (`Map`/`List`/`Set`)
//!   value, the new value is not statically known at plan time, so we
//!   **reject** the update with `fk_update_unsupported_new_value` rather than
//!   silently skipping (least-surprise: the operator explicitly declared a
//!   referential action, so dropping the fan-out would violate it).
//!
//! ## Actions
//!
//! - **Restrict** — if any child row references one of the *old* values being
//!   changed, reject (`fk_restrict`).  Mirrors `check_fk_restrict` but keyed
//!   on the about-to-change old value, not the about-to-delete row.
//! - **Cascade** — for each child row whose FK == old, UPDATE its FK field to
//!   `new_value` (NOT a delete).  New [`PendingMutation::UpdateField`].
//! - **SetNull** — for each child row whose FK == old, set the FK field to
//!   `Null` (child field must be nullable; reuses the `field_is_nullable`
//!   check).
//!
//! ## Atomicity / TOCTOU
//!
//! Same as delete: the plan is resolved before the mutation calls, and the
//! child mutations are applied **inside** the same implicit/explicit tx as
//! the parent update so the commit is atomic.  As of F-28 Step 2, the plan's
//! row-level probes (`collect_parent_values`, `any_child_references`) read via
//! `TableManager::list_stream_tx(Some(tx), ..)` — the calling transaction's
//! own snapshot-plus-staged-writes view — instead of committed-only state.
//! This closes the **in-tx** read-your-own-writes gap (D1): an old parent
//! value changed/removed earlier in the SAME transaction, or a child row
//! inserted/updated earlier in the same transaction, is now correctly
//! reflected in the RESTRICT/CASCADE/SET NULL fan-out decision.
//!
//! ## Cross-transaction race — CLOSED for the implicit path (F-28 Step 5, S3-C)
//!
//! The **cross-transaction** race — a genuinely concurrent OTHER
//! transaction's write landing between this plan and the eventual commit —
//! is now closed for the IMPLICIT update path the same way `fk_restrict.rs`
//! closes it for delete: `query_runner.rs`'s implicit UPDATE arm checks
//! `FkReverseCache::is_fk_parent_with_update_action` and opens the implicit tx as
//! `Serializable` when this table is an FK parent with a non-`NoAction`
//! `on_update` action (symmetric with the DELETE arm's upgrade — the
//! `list_stream_tx` child-table scans in `plan_fk_on_update` above are the
//! SAME shape as `fk_restrict.rs`/`fk_actions.rs`'s, so they carry the
//! identical cross-transaction race exposure and get the identical fix,
//! including a bounded internal retry for the common already-resolved-race
//! case). The child-side footprint widening
//! (`require_footprint_if_fk_child`) is unconditional — it fires on every
//! insert/update/set into an FK-child table regardless of which parent-side
//! operation (delete or update) eventually checks against it. See
//! `fk_restrict.rs`'s module doc for the full mechanism description and
//! `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs`
//! for the deterministic end-to-end proof.
//!
//! Residual scope: same as `fk_restrict.rs`/`fk_actions.rs` — closed for the
//! implicit path; an explicit tx that stays Snapshot is unaffected unless
//! the caller itself opens it Serializable.

use bytes::Bytes;
use futures::StreamExt;
use std::collections::BTreeSet;

use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::BatchError;
use shamir_query_types::filter::Filter;
use shamir_query_types::write::UpdateOp;
use shamir_types::record_view::{scalar_ref_cmp_qv, RecordRef, RecordView, ScalarRef};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::batch::TableResolver;
use crate::query::filter::{compile_filter, FilterContext, FilterNode};
use crate::query::TableRef;
use crate::repo::build_reverse_fk_entries;
use crate::table::record_cow::RecordCow;
use crate::table::TableManager;

/// A single reverse-FK reference whose `on_update != NoAction`.
pub(crate) struct OnUpdateRef {
    /// Child table name (same repo as the parent).
    child_table: String,
    /// Child field path holding the FK value (first segment for the probe).
    child_field: String,
    /// Parent field the child references (`ref_field`).
    parent_ref_field: String,
    /// The `on_update` action (Restrict / Cascade / SetNull).
    action: FkAction,
}

/// A single child mutation to apply inside the tx.
#[derive(Clone)]
enum PendingMutation {
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
/// (before the tx); applied inside the tx via [`apply_fk_update_plan`].
pub(crate) struct FkUpdatePlan {
    mutations: Vec<PendingMutation>,
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
            let new = set_fields
                .get(&rref.parent_ref_field)
                .expect("filtered above");
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

/// Apply the on-update fan-out plan inside the tx.
///
/// Executes all pending child mutations (field updates + setnull) in order.
/// No resolver needed — the plan carries pre-resolved `TableManager` handles.
pub(crate) async fn apply_fk_update_plan(
    plan: FkUpdatePlan,
    tx: &mut shamir_tx::TxContext,
    alias: &str,
) -> Result<(), BatchError> {
    if plan.is_empty() {
        return Ok(());
    }

    for mutation in plan.mutations {
        match mutation {
            PendingMutation::UpdateField {
                table,
                id,
                field,
                new_value,
            } => {
                update_child_field(&table, id, &field, &new_value, tx, alias).await?;
            }
            PendingMutation::SetNull { table, id, field } => {
                set_child_field_null(&table, id, &field, tx, alias).await?;
            }
        }
    }

    Ok(())
}

// ── Child field mutation (in-tx) ─────────────────────────────────────────────

/// Update a child row's field to `new_value` via an in-tx byte-merge.
///
/// Reads the current row bytes, replaces the field value with the new scalar,
/// and stages the update via `update_tx_bytes`.  Mirrors `set_child_field_null`
/// but with an arbitrary new scalar instead of Null.
async fn update_child_field(
    table: &TableManager,
    id: RecordId,
    field: &str,
    new_value: &QueryValue,
    tx: &mut shamir_tx::TxContext,
    alias: &str,
) -> Result<(), BatchError> {
    let old_bytes = table
        .read_one_tx_bytes(id, Some(&*tx))
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_on_update: read failed: {e}"),
            code: Some("fk_on_update".to_string()),
        })?
        .ok_or_else(|| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_on_update: row {:?} not found", id),
            code: Some("fk_on_update".to_string()),
        })?;

    let interner = table
        .interner()
        .get()
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_on_update: interner: {e}"),
            code: Some("fk_on_update".to_string()),
        })?;

    let field_id = match interner.get_ind(field) {
        Some(id) => id,
        None => {
            return Err(BatchError::query_coded(
                alias,
                "fk_on_update",
                format!("unknown field '{}'", field),
            ));
        }
    };

    let new_inner = qv_to_inner_scalar(new_value);
    let new_bytes = replace_field(&old_bytes, field_id.id(), new_inner).map_err(|e| {
        BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_on_update: merge failed: {e}"),
            code: Some("fk_on_update".to_string()),
        }
    })?;

    // #1205: `replace_field` only overwrites an existing key's value (no
    // new map keys) — not captured at this call site regardless, so leave
    // A8's decode fallback in effect (`ids_captured = false`).
    table
        .update_tx_bytes(id, &old_bytes, new_bytes, tx, false)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_on_update: update_tx_bytes failed: {e}"),
            code: Some("fk_on_update".to_string()),
        })?;

    Ok(())
}

/// Set a child row's field to Null via an in-tx update.
///
/// Mirrors `fk_actions::set_child_field_null` exactly — kept local so the
/// on-update path is self-contained and does not reach into the delete-path's
/// private helpers.
async fn set_child_field_null(
    table: &TableManager,
    id: RecordId,
    field: &str,
    tx: &mut shamir_tx::TxContext,
    alias: &str,
) -> Result<(), BatchError> {
    let old_bytes = table
        .read_one_tx_bytes(id, Some(&*tx))
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_on_update: set_null read failed: {e}"),
            code: Some("fk_on_update".to_string()),
        })?
        .ok_or_else(|| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_on_update: set_null row {:?} not found", id),
            code: Some("fk_on_update".to_string()),
        })?;

    let interner = table
        .interner()
        .get()
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_on_update: set_null interner: {e}"),
            code: Some("fk_on_update".to_string()),
        })?;

    let field_id = match interner.get_ind(field) {
        Some(id) => id,
        None => {
            return Err(BatchError::query_coded(
                alias,
                "fk_on_update",
                format!("set_null: unknown field '{}'", field),
            ));
        }
    };

    let new_bytes = replace_field(&old_bytes, field_id.id(), InnerValue::Null).map_err(|e| {
        BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_on_update: set_null merge failed: {e}"),
            code: Some("fk_on_update".to_string()),
        }
    })?;

    // #1205: same as above — `replace_field` overwrites an existing key's
    // value only, but this call site doesn't capture ids, so leave A8's
    // decode fallback in effect (`ids_captured = false`).
    table
        .update_tx_bytes(id, &old_bytes, new_bytes, tx, false)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_on_update: set_null update_tx_bytes failed: {e}"),
            code: Some("fk_on_update".to_string()),
        })?;

    Ok(())
}

/// Produce a new storage-bytes map with `field_id` set to `new_value`.
///
/// Generalization of `fk_actions::null_out_field` to any scalar `InnerValue`.
fn replace_field(
    old_bytes: &Bytes,
    field_id: u64,
    new_value: InnerValue,
) -> Result<Bytes, shamir_storage::error::DbError> {
    let mut tree = InnerValue::from_bytes(old_bytes.clone()).map_err(|e| {
        shamir_storage::error::DbError::Codec(format!("fk_on_update replace_field decode: {e}"))
    })?;
    if let InnerValue::Map(ref mut m) = tree {
        let key = shamir_types::core::interner::InternerKey::new(field_id);
        m.insert(key, new_value);
    }
    tree.to_bytes().map_err(|e| {
        shamir_storage::error::DbError::Codec(format!("fk_on_update replace_field encode: {e}"))
    })
}

async fn field_is_nullable(table: &TableManager, field: &str) -> bool {
    let bindings = table.validator_bindings();
    let registry = match table.validator_registry_ref() {
        Some(r) => r,
        None => return false, // No registry → can't confirm nullable → fail closed.
    };

    for binding in bindings.iter() {
        let validator = match registry.get_by_id(&binding.validator_id) {
            Some(v) => v,
            None => continue,
        };
        if let Some(nullable) = validator.nullable_for_field(field) {
            return nullable;
        }
    }

    // No validator declared this field → assume NOT nullable (fail closed).
    false
}

// ── Discovery ────────────────────────────────────────────────────────────────

/// Discover all child tables in the repo that have a FK pointing at
/// `parent_table` with `on_update != NoAction`, already narrowed to the
/// references relevant to THIS update's `set_fields`.
///
/// (#1181): reads through the repo's cached reverse-FK map
/// (`RepoInstance::fk_reverse_cache`) instead of re-running the O(tables)
/// `repo.list_table_names()` + `collect_fk_refs()` scan on every UPDATE —
/// mirrors `fk_actions::discover_action_refs` / `fk_restrict::discover_restrict_refs`,
/// sharing the SAME cache (and the same underlying scan): a cache miss (never
/// built, or invalidated by a schema mutation since the last build) pays that
/// scan exactly once and seeds every parent table's entry in the repo, not
/// just this one. The cache carries every `on_update` action; this filters
/// for what the UPDATE path needs.
///
/// The set-fields membership check (`set_fields.contains_key`) is applied as
/// the FIRST predicate in the filter chain, before the `on_update` check and
/// before allocating an owned [`OnUpdateRef`] (three `String` clones) — for
/// the common no-op update (touching no FK-referenced field), every cached
/// entry is rejected by this cheap local-map lookup alone.
pub(crate) async fn discover_on_update_refs(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
    set_fields: &shamir_collections::TFxMap<String, QueryValue>,
) -> Result<Vec<OnUpdateRef>, BatchError> {
    let repo = resolver
        .resolve_repo(&parent_table_ref.repo)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("fk_on_update: resolve_repo({}): {e}", parent_table_ref.repo),
            code: Some("fk_on_update".to_string()),
        })?;

    let repo_name = parent_table_ref.repo.clone();
    let all_for_parent = repo
        .fk_reverse_cache()
        .get_or_build_by_parent(&parent_table_ref.table, || {
            // F-36: `Fn` (re-callable) build closure so a
            // generation-mismatched scan can retry under the same
            // single-flight lock. Clone `repo_name` per call.
            let repo_name = repo_name.clone();
            async move { build_reverse_fk_entries(resolver, &repo_name).await }
        })
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("fk_on_update: reverse-FK scan failed: {e}"),
            code: Some("fk_on_update".to_string()),
        })?;

    Ok(all_for_parent
        .iter()
        .filter(|e| set_fields.contains_key(&e.parent_ref_field))
        .filter(|e| e.on_update != FkAction::NoAction)
        .map(|e| OnUpdateRef {
            child_table: e.child_table.clone(),
            child_field: e.child_field.clone(),
            parent_ref_field: e.parent_ref_field.clone(),
            action: e.on_update,
        })
        .collect())
}

// ── Scan helpers (mirrors fk_restrict.rs / fk_actions.rs) ────────────────────

/// Scan the parent table for rows matching `where_clause` and extract the
/// DISTINCT values of the given `ref_fields`.
///
/// `None` for `where_clause` means match-all (mirrors `execute_update_tx`'s
/// treatment of an absent WHERE).
///
/// Returns a map: `field_name -> TFxSet<QueryValue>` — deduped, so that many
/// matched parent rows sharing the same ref_field value collapse to one
/// downstream probe instead of one per row (previously this pushed every
/// matched row's value into a `Vec`, including duplicates, despite the doc
/// claiming "distinct values").
async fn collect_parent_values(
    table: &TableManager,
    where_clause: Option<&Filter>,
    ctx: &FilterContext<'_>,
    ref_fields: &[&str],
    tx: &shamir_tx::TxContext,
) -> shamir_storage::error::DbResult<
    shamir_collections::TFxMap<String, shamir_collections::TFxSet<QueryValue>>,
> {
    let interner = table.interner().get().await?;
    let callback = where_clause
        .map(|f| compile_filter(f, interner))
        .unwrap_or(FilterNode::True);
    let batch_size = 1000;

    let mut result: shamir_collections::TFxMap<String, shamir_collections::TFxSet<QueryValue>> =
        shamir_collections::TFxMap::default();
    for &field in ref_fields {
        result.insert(field.to_string(), shamir_collections::TFxSet::default());
    }

    // F-28 Step 2: resolve each ref_field's id through the tx-layered
    // interner (base, then this tx's `interner_overlay`) once, up front —
    // a ref_field touched for the first time earlier in this SAME tx only
    // exists in the overlay until commit; a base-only lookup would silently
    // drop it from every matched row's extracted values.
    let mut field_ids: shamir_collections::TFxMap<&str, u64> =
        shamir_collections::TFxMap::default();
    for &field in ref_fields {
        if let Some(id) = resolve_field_id_layered(interner, tx, field).await {
            field_ids.insert(field, id);
        }
    }

    // F-28 Step 2: tx-aware scan — overlays this tx's own staged writes so an
    // old parent value changed/removed EARLIER in the same tx is correctly
    // hidden/reflected (read-your-own-writes), closing D1 for this probe.
    let stream = table.list_stream_tx(Some(tx), batch_size);
    futures::pin_mut!(stream);
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        for (_id, cow) in batch {
            let bytes: Bytes = cow_to_bytes(cow)?;

            if !filter_matches(&bytes, &callback, ctx) {
                continue;
            }

            for &field in ref_fields {
                if let Some(&field_id) = field_ids.get(field) {
                    let key = shamir_types::core::interner::InternerKey::new(field_id);
                    let path = std::slice::from_ref(&key);
                    // Decode-corrupt row: fail closed instead of silently
                    // dropping this row's value from the RESTRICT/CASCADE/
                    // SET NULL candidate set (decode-error twin of the
                    // F-65/#891 read-error fix — see
                    // `extract_ref_field_value`'s doc).
                    if let Some(qv) = extract_ref_field_value(&bytes, path)? {
                        result.get_mut(field).unwrap().insert(qv);
                    }
                }
            }
        }
    }

    Ok(result)
}

/// Decode a row's bytes (`RecordView`, falling back to `InnerValue` for
/// non-map records — same two-step decode `record_field_matches_by_id`
/// uses) and extract the scalar at `path` as an owned `QueryValue`.
///
/// Returns `Ok(None)` when the record decodes fine but the field is absent
/// at `path` — a legitimate "this row doesn't have that field" outcome, not
/// corruption. Returns `Err(DbError::Codec)` only when BOTH decode attempts
/// fail on the raw bytes: a genuinely corrupt parent row must abort the
/// scan, not be silently dropped from the candidate value set — the
/// decode-error twin of the F-65/#891 read-error fail-closed fix (see
/// `fk_indexed_action_read_error_tests.rs`).
fn extract_ref_field_value(
    bytes: &Bytes,
    path: &[shamir_types::core::interner::InternerKey],
) -> shamir_storage::error::DbResult<Option<QueryValue>> {
    if let Ok(view) = RecordView::new(bytes) {
        return Ok(view.scalar_at(path).map(scalar_ref_to_qv));
    }
    match InnerValue::from_bytes(bytes.clone()) {
        Ok(tree) => Ok(tree.scalar_at(path).map(scalar_ref_to_qv)),
        Err(e) => Err(shamir_storage::error::DbError::Codec(format!(
            "fk_on_update: parent row decode failed (RecordView and InnerValue \
             both failed): {e}"
        ))),
    }
}

/// Check whether any row in `child_table` has `field` equal to ANY member of
/// `values` — a single pass over the child table testing the whole (deduped)
/// old-value set for one FK field.
///
/// Replaces the previous per-value `child_has_reference`, which was called
/// once PER (old-value, child_field) pair from the RESTRICT branch above
/// and, whenever the field lacked a supporting index, ran a full
/// `list_stream_tx` scan of the child table EVERY time — O(distinct values ×
/// child-table size) instead of O(child-table size). The per-value INDEXED
/// lookup path is unchanged in shape (one `lookup_by_index` call per value —
/// already O(log n) each, not the fan-out this fixes); only the no-index
/// fallback collapses to exactly one scan, and each scanned row is decoded
/// exactly once and tested against every value instead of being
/// independently re-scanned-and-decoded once per value.
///
/// Mirrors `fk_restrict::any_child_references`: index-fast / scan-fallback,
/// PLUS (F-28 Step 2) a staged-overlay probe on the index fast-path's empty
/// result, since a staged-but-not-yet-committed insert/update is never
/// reflected in the index (indexing happens at commit).
async fn any_child_references(
    table: &TableManager,
    field: &str,
    values: &[&QueryValue],
    tx: &shamir_tx::TxContext,
) -> shamir_storage::error::DbResult<bool> {
    // F-40b (RI barrier): record this child-table scan REGARDLESS of
    // isolation — mirrors `fk_restrict.rs::any_child_references`'s recording
    // at function entry, before the index fast-path, so an EXPLICIT
    // `Snapshot`-isolation parent UPDATE (which never populates
    // `predicate_set`) still gets a commit-time re-check (`pre_commit.rs`
    // Phase 2-bis) against concurrent committers that touched this child
    // table. Fires even when a supporting index short-circuits the scan
    // below.
    tx.record_ri_barrier(table.table_token());

    let interner = table.interner().get().await?;

    // F-28 Step 2: resolve the field id through the tx-layered interner
    // (base, then this tx's `interner_overlay`) — a field first interned
    // EARLIER IN THE SAME tx only exists in the overlay until commit; a
    // base-only lookup would silently miss it. Mirrors
    // `fk_restrict::any_child_references`.
    let field_id = resolve_field_id_layered(interner, tx, field).await;

    // Fast path: one `lookup_by_index` call per value (only meaningful for a
    // base id). Falls through to the single full-scan fallback below the
    // moment ANY value can't be resolved via the index — matching the old
    // per-value function's per-call behavior exactly.
    if let Some(base_field_id) = interner.get_ind(field) {
        let field_path = [base_field_id.id()];
        if let Some(idx_name) = table.find_single_field_index(&field_path) {
            let mut index_conclusive = true;
            for &value in values {
                let Some(inner_value) = qv_scalar_to_inner(value) else {
                    index_conclusive = false;
                    break;
                };
                // P0-3a (#1011): `None` = DROP in its drain→sweep window — the
                // index is momentarily unusable, so fall through to the full
                // scan below instead of treating it as "no match" (which would
                // wrongly let a FK-referencing child row pass the check).
                match table
                    .index_manager_ref()
                    .lookup_by_index(idx_name, std::slice::from_ref(&inner_value))
                    .await?
                {
                    Some(ids) => {
                        if !ids.is_empty() {
                            return Ok(true);
                        }
                        // Index says no match — authoritative for the COMMITTED
                        // store, but cannot rule out a same-tx staged insert/update.
                        if let Some(field_id) = field_id {
                            let key = shamir_types::core::interner::InternerKey::new(field_id);
                            if staged_field_matches(tx, table.table_token(), &key, value) {
                                return Ok(true);
                            }
                        }
                    }
                    None => {
                        index_conclusive = false;
                        break;
                    }
                }
            }
            if index_conclusive {
                return Ok(false);
            }
            // else: fall through to the full scan for correctness (it
            // harmlessly re-checks any value already ruled out via index).
        }
    }

    // Fallback: ONE full scan with field match against the WHOLE `values`
    // set — each row is decoded exactly once and tested against every
    // value, instead of once per value. Tx-aware: overlays this tx's own
    // staged writes (read-your-own-writes, closing D1 for this probe).
    //
    // F-28 Step 5 (memo §2.4): construct `list_stream_tx` UNCONDITIONALLY —
    // it records the Serializable `TableScan` predicate synchronously at
    // construction time (before this line returns), regardless of whether
    // `field_id` resolved. See the identical fix + rationale in
    // `fk_restrict.rs::any_child_references`.
    let batch_size = 1000;
    let stream = table.list_stream_tx(Some(tx), batch_size);
    let Some(field_id) = field_id else {
        return Ok(false);
    };
    let key = shamir_types::core::interner::InternerKey::new(field_id);
    futures::pin_mut!(stream);
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        for (_, cow) in batch {
            let bytes: Bytes = cow_to_bytes(cow)?;
            if record_field_matches_any_by_id(bytes.as_ref(), &key, values) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// F-28 Step 2: resolve `field` to its interner id, checking `base` first
/// and then this tx's `interner_overlay`. See the identical helper + doc
/// comment in `fk_actions.rs` / `fk_restrict.rs` for the full rationale.
async fn resolve_field_id_layered(
    base: &shamir_types::core::interner::Interner,
    tx: &shamir_tx::TxContext,
    field: &str,
) -> Option<u64> {
    let layered = shamir_tx::LayeredInterner::Layered {
        base,
        overlay: &tx.interner_overlay,
        next_overlay_id: &tx.next_overlay_id,
    };
    layered.get_id(field).await
}

/// F-28 Step 2: does this tx's own staged (uncommitted) write_set for
/// `target_token` contain a row where `field_id == value`? Mirrors
/// `fk_restrict::staged_field_matches` / `ValidatorDb::staged_field_matches`.
fn staged_field_matches(
    tx: &shamir_tx::TxContext,
    target_token: u64,
    field_id: &shamir_types::core::interner::InternerKey,
    value: &QueryValue,
) -> bool {
    let Some(staging) = tx.write_set.get(&target_token) else {
        return false;
    };
    // P1 perf fix: called once PER RECORD in a batch FK on-update check —
    // `iter_ops()` avoids materializing a fresh `Vec<KvOp>` on every call
    // (`snapshot_ops()` would make this O(staged²) across the batch) and lets
    // `.any()` short-circuit on the first match.
    staging.iter_ops().any(|op| match op {
        shamir_storage::types::KvOp::Set(_, ref bytes) => {
            record_field_matches_by_id(bytes.as_ref(), field_id, value)
        }
        shamir_storage::types::KvOp::Remove(_) => false,
    })
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn cow_to_bytes(cow: RecordCow) -> shamir_storage::error::DbResult<Bytes> {
    match cow {
        RecordCow::Borrowed(b) => Ok(b),
        RecordCow::Owned(tree) => tree.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("fk_on_update scan serialize: {e}"))
        }),
    }
}

fn filter_matches(bytes: &Bytes, callback: &FilterNode, ctx: &FilterContext<'_>) -> bool {
    match RecordView::new(bytes) {
        Ok(view) => callback.matches(&view, ctx),
        Err(_) => {
            if let Ok(tree) = InnerValue::from_bytes(bytes.clone()) {
                callback.matches(&tree, ctx)
            } else {
                false
            }
        }
    }
}

fn scalar_ref_matches_qv(actual: &ScalarRef<'_>, value: &QueryValue) -> bool {
    // Delegate to the cross-type-comparing `scalar_ref_cmp_qv` so that a parent
    // key stored as `Int(5)` matches a child FK field stored as `F64(5.0)` (and
    // vice-versa) — consistent with every other comparison layer in the engine.
    scalar_ref_cmp_qv(*actual, value) == Some(std::cmp::Ordering::Equal)
}

/// Keyed by an already-resolved `InternerKey` instead of a field name — the
/// field id must come from [`resolve_field_id_layered`] (base + this tx's
/// overlay), not a plain by-name interner lookup, since the matched row may
/// be tx-overlay-sourced (`list_stream_tx`) and use an overlay-only field id.
fn record_field_matches_by_id(
    record_bytes: &[u8],
    field_id: &shamir_types::core::interner::InternerKey,
    value: &QueryValue,
) -> bool {
    let path = std::slice::from_ref(field_id);
    if let Ok(view) = RecordView::new(record_bytes) {
        if let Some(actual) = view.scalar_at(path) {
            return scalar_ref_matches_qv(&actual, value);
        }
    }
    if let Ok(tree) = InnerValue::from_bytes(Bytes::copy_from_slice(record_bytes)) {
        if let Some(actual) = tree.scalar_at(path) {
            return scalar_ref_matches_qv(&actual, value);
        }
    }
    false
}

/// Like [`record_field_matches_by_id`], but tests a SINGLE decoded row
/// against every member of `values` — the row is decoded exactly once
/// regardless of how many values are probed, closing the redundant
/// per-probe re-decode `any_child_references`'s old per-value shape caused.
fn record_field_matches_any_by_id(
    record_bytes: &[u8],
    field_id: &shamir_types::core::interner::InternerKey,
    values: &[&QueryValue],
) -> bool {
    let path = std::slice::from_ref(field_id);
    if let Ok(view) = RecordView::new(record_bytes) {
        if let Some(actual) = view.scalar_at(path) {
            return values.iter().any(|v| scalar_ref_matches_qv(&actual, v));
        }
    }
    if let Ok(tree) = InnerValue::from_bytes(Bytes::copy_from_slice(record_bytes)) {
        if let Some(actual) = tree.scalar_at(path) {
            return values.iter().any(|v| scalar_ref_matches_qv(&actual, v));
        }
    }
    false
}

/// Row decoded once for testing against multiple probes — see `decode_row`.
enum DecodedRow<'a> {
    View(RecordView<'a>),
    Tree(InnerValue),
}

impl DecodedRow<'_> {
    fn scalar_at(
        &self,
        path: &[shamir_types::core::interner::InternerKey],
    ) -> Option<ScalarRef<'_>> {
        match self {
            DecodedRow::View(v) => v.scalar_at(path),
            DecodedRow::Tree(t) => t.scalar_at(path),
        }
    }
}

/// Decode `bytes` once via `RecordView`, falling back to `InnerValue` for
/// non-map records — shared across every probe tested against the SAME row
/// (closes the redundant per-probe re-decode `record_field_matches_by_id`
/// caused when invoked once per probe for one row in `classify_row_update`).
fn decode_row(bytes: &[u8]) -> Option<DecodedRow<'_>> {
    if let Ok(view) = RecordView::new(bytes) {
        return Some(DecodedRow::View(view));
    }
    if let Ok(tree) = InnerValue::from_bytes(Bytes::copy_from_slice(bytes)) {
        return Some(DecodedRow::Tree(tree));
    }
    None
}

/// F-53c: per-row fan-out resolution shared by the index fast-path and the
/// `list_stream_tx` scan fallback in `plan_fk_on_update`. Mutates `mutations`
/// exactly as the original inline loop did: the UPDATE path is PER-FIELD (a
/// single row may carry two distinct FK fields that both reference the old
/// value, and each must be re-keyed or nulled according to its own FK's
/// action). Extracting this into a shared helper guarantees the two paths
/// produce byte-for-byte identical results whether or not a supporting
/// index exists on the FK column.
fn classify_row_update(
    id: RecordId,
    bytes: &[u8],
    resolved_probes: &[(&QueryValue, &QueryValue, FkAction, &str, u64)],
    child_table: &TableManager,
    mutations: &mut Vec<PendingMutation>,
) {
    // Decode the row ONCE regardless of how many probes it's tested
    // against — `record_field_matches_by_id` previously re-decoded these
    // SAME bytes from scratch on every probe iteration below.
    let Some(row) = decode_row(bytes) else {
        // Corrupt row: every probe below would already have failed its own
        // decode identically, so "no match for this row" is the same
        // outcome as before.
        return;
    };
    // For this row, apply EVERY matching probe.  A field is a single FK with
    // a single action, and a row's field value can equal at most one `old`
    // value, so at most one probe per field matches a given row — there is
    // no per-field Cascade/SetNull conflict to resolve.
    for (old, new, action, field, field_id) in resolved_probes {
        let key = shamir_types::core::interner::InternerKey::new(*field_id);
        let path = std::slice::from_ref(&key);
        let matches = row
            .scalar_at(path)
            .is_some_and(|actual| scalar_ref_matches_qv(&actual, old));
        if !matches {
            continue;
        }
        match action {
            FkAction::Cascade => {
                mutations.push(PendingMutation::UpdateField {
                    table: child_table.clone(),
                    id,
                    field: field.to_string(),
                    new_value: (*new).clone(),
                });
            }
            FkAction::SetNull => {
                mutations.push(PendingMutation::SetNull {
                    table: child_table.clone(),
                    id,
                    field: field.to_string(),
                });
            }
            _ => {}
        }
    }
}

/// F-53c: index fast-path candidate gathering, mirroring
/// `fk_restrict::child_has_reference`'s index lookup exactly (and the
/// identical helper in `fk_actions.rs`). See `fk_actions::index_candidate_ids`
/// for the full contract. The caller MUST have already recorded the RI-barrier
/// token at entry (before this call), preserving the F-40b mutual-serialization
/// ordering exactly as `child_has_reference` does.
async fn index_candidate_ids(
    table: &TableManager,
    interner: &shamir_types::core::interner::Interner,
    probes: &[(&str, &QueryValue)],
) -> shamir_storage::error::DbResult<Option<Vec<RecordId>>> {
    let mut all: BTreeSet<RecordId> = BTreeSet::new();
    for (field, value) in probes {
        let inner = match qv_scalar_to_inner(value) {
            Some(InnerValue::Null) | None => return Ok(None),
            Some(iv) => iv,
        };
        let base_field_id = match interner.get_ind(field) {
            Some(id) => id,
            None => return Ok(None),
        };
        let field_path = [base_field_id.id()];
        let idx_name = match table.find_single_field_index(&field_path) {
            Some(n) => n,
            None => return Ok(None),
        };
        let ids = match table
            .index_manager_ref()
            .lookup_by_index(idx_name, std::slice::from_ref(&inner))
            .await?
        {
            // P0-3a (#1011): `None` = a DROP of this index is in its drain→sweep
            // window — index momentarily unusable. Bail to a full scan
            // (`Ok(None)`) instead of treating it as "no matches".
            Some(ids) => ids,
            None => return Ok(None),
        };
        all.extend(ids.iter().copied());
    }
    Ok(Some(all.into_iter().collect()))
}

fn scalar_ref_to_qv(sr: ScalarRef<'_>) -> QueryValue {
    match sr {
        ScalarRef::Null => QueryValue::Null,
        ScalarRef::Bool(b) => QueryValue::Bool(b),
        ScalarRef::Int(i) => QueryValue::Int(i),
        ScalarRef::F64(f) => QueryValue::F64(f),
        ScalarRef::Str(s) => QueryValue::Str(s.to_owned()),
        ScalarRef::Bin(b) => QueryValue::Bin(b.to_vec()),
    }
}

fn qv_scalar_to_inner(qv: &QueryValue) -> Option<InnerValue> {
    match qv {
        QueryValue::Null => Some(InnerValue::Null),
        QueryValue::Bool(b) => Some(InnerValue::Bool(*b)),
        QueryValue::Int(i) => Some(InnerValue::Int(*i)),
        QueryValue::F64(f) => Some(InnerValue::F64(*f)),
        QueryValue::Str(s) => Some(InnerValue::Str(s.clone())),
        QueryValue::Bin(b) => Some(InnerValue::Bin(b.clone())),
        QueryValue::Dec(d) => Some(InnerValue::Dec(*d)),
        QueryValue::Big(b) => Some(InnerValue::Big(b.clone())),
        _ => None,
    }
}

/// Convert a query-value scalar to its inner (interner-keyed) form for the
/// storage byte-merge.  Non-scalar types fall back to Null (they are rejected
/// earlier by `qv_is_supported_scalar`).
fn qv_to_inner_scalar(qv: &QueryValue) -> InnerValue {
    qv_scalar_to_inner(qv).unwrap_or(InnerValue::Null)
}

/// A `new_value` we can propagate at plan time: a literal scalar.
///
/// `Map` / `List` / `Set` are non-scalar; the `$fn`/`$ref`/`$expr` markers
/// surface here as `Map`/`List` (the reference carrier shape), so this guard
/// rejects them too.
fn qv_is_supported_scalar(qv: &QueryValue) -> bool {
    matches!(
        qv,
        QueryValue::Null
            | QueryValue::Bool(_)
            | QueryValue::Int(_)
            | QueryValue::F64(_)
            | QueryValue::Str(_)
            | QueryValue::Bin(_)
            | QueryValue::Dec(_)
            | QueryValue::Big(_)
    )
}

fn action_name(a: FkAction) -> &'static str {
    match a {
        FkAction::NoAction => "no_action",
        FkAction::Restrict => "restrict",
        FkAction::Cascade => "cascade",
        FkAction::SetNull => "set_null",
    }
}
