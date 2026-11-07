use bytes::Bytes;
use futures::StreamExt;

use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::BatchError;
use shamir_query_types::filter::Filter;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::query::batch::TableResolver;
use crate::query::filter::FilterContext;
use crate::query::TableRef;
use crate::repo::ReverseFkEntry;
use crate::table::TableManager;

use super::helpers::{
    classify_row, collect_parent_values, cow_to_bytes, discover_action_refs,
    extract_ref_field_value, field_is_nullable, index_candidate_ids, resolve_field_id_layered,
};

///
/// This is a safety valve against FK cycles (A→B→A).  Real cascade chains are
/// shallow (typically 2–3 levels); 32 is generous while still bounding the
/// worst case.
pub(crate) const CASCADE_DEPTH_LIMIT: usize = 32;

/// A single mutation to apply inside the tx.
#[derive(Clone)]
pub(super) enum PendingMutation {
    /// Delete this child row (Cascade).
    Delete { table: TableManager, id: RecordId },
    /// Set this child row's FK field to Null (SetNull).
    SetNull {
        table: TableManager,
        id: RecordId,
        field: String,
    },
}

/// A fully-resolved cascade/setnull plan — a flat list of mutations to apply
/// inside the tx, in order.  Built at planning time (before the tx) by
/// recursively walking the cascade tree.
pub(crate) struct CascadePlan {
    pub(super) mutations: Vec<PendingMutation>,
}

impl CascadePlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.mutations.is_empty()
    }
}

/// RAII guard for **per-path** cascade cycle detection.
///
/// Inserts the table name into the `visited` set on construction (returning
/// `Err(fk_cascade_depth)` if the table is already on the current recursion
/// path — a genuine cycle like `A→B→A`), and **removes** the entry on [`Drop`].
/// Because the entry is removed whenever the branch that inserted it returns —
/// whether via `Ok`, an early `?`, or a propagated `Err` — a table reachable
/// via two distinct non-cyclic paths (a legal diamond/DAG FK topology) is no
/// longer mistaken for a cycle.  The entry thus lives only while the branch
/// that inserted it is on the call stack.
///
/// Callers that need to recurse pass [`CascadePathGuard::visited_mut`] (a
/// reborrow) to the callee, which constructs its own guard from that reborrow.
/// The [`CASCADE_DEPTH_LIMIT`] check remains as a secondary safety valve.
struct CascadePathGuard<'a> {
    visited: &'a mut shamir_collections::TFxSet<String>,
    table: String,
}

impl<'a> CascadePathGuard<'a> {
    /// Mark `table` as on the current cascade path.  Returns `Err` if the
    /// table is already present (genuine cycle on this path).
    fn enter(
        visited: &'a mut shamir_collections::TFxSet<String>,
        table: String,
        alias: &str,
    ) -> Result<Self, BatchError> {
        if visited.insert(table.clone()) {
            Ok(Self { visited, table })
        } else {
            Err(BatchError::query_coded(
                alias,
                "fk_cascade_depth",
                format!(
                    "cascade cycle detected at table '{}'; possible FK cycle",
                    table
                ),
            ))
        }
    }

    /// Reborrow the visited set so a recursive call can construct its own
    /// [`CascadePathGuard`].  The reborrow is released when the callee
    /// returns, leaving the caller's guard (and its `visited` entry) intact.
    fn visited_mut(&mut self) -> &mut shamir_collections::TFxSet<String> {
        self.visited
    }
}

impl Drop for CascadePathGuard<'_> {
    fn drop(&mut self) {
        self.visited.remove(&self.table);
    }
}

/// Row-level dedup state for cascade mutation scheduling.
///
/// Prevents the same row from being scheduled twice when it is reachable via
/// two distinct cascade paths (a diamond/DAG topology).  Deletes are deduped
/// by `(table, id)`; SetNulls by `(table, id, field)` so that nulling two
/// different FK fields of the same row is still allowed.  A SetNull for a row
/// already scheduled for Delete is skipped — the row will be removed, making
/// the null pointless and potentially erroring if applied post-delete.
struct MutationDedup {
    deletes: shamir_collections::TFxSet<(String, RecordId)>,
    setnulls: shamir_collections::TFxSet<(String, RecordId, String)>,
}

impl MutationDedup {
    fn new() -> Self {
        Self {
            deletes: shamir_collections::TFxSet::default(),
            setnulls: shamir_collections::TFxSet::default(),
        }
    }

    /// Returns `true` if this is the first Delete scheduled for `(table, id)`.
    fn try_delete(&mut self, table: &str, id: RecordId) -> bool {
        self.deletes.insert((table.to_string(), id))
    }

    /// Returns `true` if this SetNull should be scheduled: the row is not
    /// already slated for Delete and this `(table, id, field)` triple hasn't
    /// been scheduled yet.
    fn try_set_null(&mut self, table: &str, id: RecordId, field: &str) -> bool {
        if self.deletes.contains(&(table.to_string(), id)) {
            return false;
        }
        self.setnulls
            .insert((table.to_string(), id, field.to_string()))
    }
}

/// Discover + resolve the full cascade/setnull plan.
///
/// Walks the cascade tree recursively (up to [`CASCADE_DEPTH_LIMIT`]) at
/// planning time, collecting all child/grandchild rows that need to be
/// deleted or nulled.  Returns a flat [`CascadePlan`] that the caller drives
/// inside the tx via `apply_cascade_plan`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn plan_cascade(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
    parent_table: &TableManager,
    delete_where: &Filter,
    ctx: &FilterContext<'_>,
    alias: &str,
    tx: &shamir_tx::TxContext,
) -> Result<CascadePlan, BatchError> {
    let mut mutations = Vec::new();
    let mut visited: shamir_collections::TFxSet<String> = shamir_collections::TFxSet::default();
    let mut dedup = MutationDedup::new();

    plan_cascade_recursive(
        resolver,
        parent_table_ref,
        parent_table,
        delete_where,
        ctx,
        alias,
        0,
        &mut mutations,
        &mut dedup,
        &mut visited,
        tx,
    )
    .await?;

    Ok(CascadePlan { mutations })
}

/// One level of cascade recursion.
///
/// `depth` is the current recursion depth (0 = direct children of the
/// originally-deleted parent).  Exceeding [`CASCADE_DEPTH_LIMIT`] rejects
/// with `fk_cascade_depth`.
#[allow(clippy::too_many_arguments)]
async fn plan_cascade_recursive(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
    parent_table: &TableManager,
    delete_where: &Filter,
    ctx: &FilterContext<'_>,
    alias: &str,
    depth: usize,
    mutations: &mut Vec<PendingMutation>,
    dedup: &mut MutationDedup,
    visited: &mut shamir_collections::TFxSet<String>,
    tx: &shamir_tx::TxContext,
) -> Result<(), BatchError> {
    if depth >= CASCADE_DEPTH_LIMIT {
        return Err(BatchError::query_coded(
            alias,
            "fk_cascade_depth",
            format!(
                "cascade recursion exceeded depth limit ({CASCADE_DEPTH_LIMIT}); \
                 possible FK cycle"
            ),
        ));
    }

    // Per-path cycle guard: a table already on the *current* recursion path
    // means a genuine FK cycle (e.g. X→Y→X).  The [`CascadePathGuard`] removes
    // the entry on drop — whether this branch returns `Ok` or `Err` — so that
    // a descendant reachable via two distinct non-cyclic paths (a legal
    // diamond/DAG topology) is not mistaken for a cycle.  The depth limit
    // above is the secondary safety valve.
    let mut path_guard = CascadePathGuard::enter(visited, parent_table_ref.table.clone(), alias)?;

    // 1. Discover reverse-FK references with Cascade or SetNull.
    let action_refs = discover_action_refs(resolver, parent_table_ref).await?;
    if action_refs.is_empty() {
        return Ok(());
    }

    // 2. Collect distinct parent ref_field values from rows about to be deleted.
    let parent_ref_fields: Vec<&str> = {
        let mut fields: Vec<&str> = action_refs
            .iter()
            .map(|r| r.parent_ref_field.as_str())
            .collect();
        fields.sort_unstable();
        fields.dedup();
        fields
    };

    let parent_values =
        collect_parent_values(parent_table, delete_where, ctx, &parent_ref_fields, tx)
            .await
            .map_err(|e| BatchError::QueryError {
                alias: alias.to_string(),
                message: format!("fk_actions: parent scan failed: {e}"),
                code: Some("fk_actions".to_string()),
            })?;

    if parent_values.values().all(|v| v.is_empty()) {
        return Ok(());
    }

    // 3. Group refs by child table and resolve handles.
    let mut by_table: shamir_collections::TFxMap<String, Vec<ReverseFkEntry>> =
        shamir_collections::TFxMap::default();
    for rref in action_refs {
        by_table
            .entry(rref.child_table.clone())
            .or_default()
            .push(rref);
    }

    for (child_name, refs) in by_table {
        // Skip if none of this child's refs have a non-empty parent value set.
        let has_values = refs.iter().any(|r| {
            parent_values
                .get(r.parent_ref_field.as_str())
                .is_some_and(|v| !v.is_empty())
        });
        if !has_values {
            continue;
        }

        let child_table_ref = TableRef::with_repo(&parent_table_ref.repo, &child_name);
        let child_table =
            resolver
                .resolve(&child_table_ref)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!(
                        "fk_actions: cannot resolve child table '{}': {e}",
                        child_name
                    ),
                    code: Some("fk_actions".to_string()),
                })?;

        // Build the probes for this child.
        let mut probes: Vec<(&str, &QueryValue, FkAction)> = Vec::new();
        for r in &refs {
            if let Some(vals) = parent_values.get(r.parent_ref_field.as_str()) {
                for v in vals {
                    probes.push((r.child_field.as_str(), v, r.on_delete));
                }
            }
        }
        if probes.is_empty() {
            continue;
        }

        // Scan child rows, collecting matching IDs.
        let interner = child_table
            .interner()
            .get()
            .await
            .map_err(|e| BatchError::QueryError {
                alias: alias.to_string(),
                message: format!("fk_actions: child interner: {e}"),
                code: Some("fk_actions".to_string()),
            })?;

        // F-28 Step 2: resolve each probe's field id through the TX-LAYERED
        // interner (base, then this tx's `interner_overlay`) — NOT base
        // alone. A child row staged (inserted/updated) earlier in the SAME
        // tx may use a field name that was interned for the first time in
        // this tx (never in base yet, only in `tx.interner_overlay`); a
        // base-only `interner.get_ind` lookup would return `None` for it and
        // silently fail to match, even though `list_stream_tx` above already
        // makes the staged row itself visible. Mirrors
        // `ValidatorDb::exists_in_self`'s staged-writes step.
        let mut resolved_probes: Vec<(u64, &QueryValue, FkAction, &str)> =
            Vec::with_capacity(probes.len());
        for (field, value, action) in &probes {
            if let Some(field_id) = resolve_field_id_layered(interner, tx, field).await {
                resolved_probes.push((field_id, value, *action, field));
            }
        }
        // F-28 Step 5 (memo §2.4): do NOT `continue` here when
        // `resolved_probes` is empty (e.g. every probed field was never
        // interned — a brand-new/empty child table). `list_stream_tx` below
        // records the Serializable `TableScan` predicate at CONSTRUCTION
        // time, before it is even polled; skipping the scan entirely would
        // skip recording that predicate too, silently reopening the
        // cross-transaction race for exactly the common case of a young
        // child table. When `resolved_probes` is empty the per-row match
        // loop below is naturally a no-op (0 iterations), so running the
        // scan costs a harmless full pass with no matches — cheap insurance
        // that keeps the SSI predicate recorded either way.

        let mut cascade_ids: Vec<RecordId> = Vec::new();
        let mut setnull_ids: Vec<(RecordId, String)> = Vec::new();

        // F-40b (RI barrier): record this child-table scan at ENTRY,
        // regardless of isolation, so an EXPLICIT `Snapshot`-isolation
        // parent delete (which never populates `predicate_set`) still gets a
        // commit-time re-check via `pre_commit.rs` Phase 2-bis against
        // concurrent committers that touched this child table. Mirrors
        // `fk_restrict.rs::child_has_reference`'s recording exactly.
        //
        // Recorded BEFORE the index fast-path branch below so it fires
        // regardless of which sub-path executes — the F-40b mutual-
        // serialization guarantee holds whether the index fast-path
        // short-circuits the scan or the `list_stream_tx` fallback runs.
        tx.record_ri_barrier(child_table.table_token());

        // F-53c: index fast-path (mirrors `child_has_reference`'s index
        // lookup for RESTRICT). When every probe's FK field resolves to a
        // BASE interner id with a single-field index AND the current tx has
        // no staged writes against this child table, fetch candidate
        // RecordIds directly via `lookup_by_index` instead of streaming +
        // matching every row. The "no staged writes" gate is the
        // correctness boundary for CASCADE/SET NULL: the index reflects
        // committed state only, so staged inserts/updates/deletes
        // (invisible to the index) must be handled by the tx-aware scan
        // fallback below. RESTRICT can consult the staged overlay piecemeal
        // (it only needs a bool); CASCADE/SET NULL need the full ID set, so
        // the clean correctness line is "use the index only when there is
        // nothing staged to reconcile".
        let index_probes: Vec<(&str, &QueryValue)> = resolved_probes
            .iter()
            .map(|(_, value, _, field)| (*field, *value))
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
                    message: format!("fk_actions: child index lookup failed: {e}"),
                    code: Some("fk_actions".to_string()),
                })?
        } else {
            None
        };

        if let Some(candidate_ids) = fast_path {
            // Index-verified candidate set (committed-only; no staged writes
            // to reconcile). Re-read each candidate and run the SAME
            // probe-matching inner loop as the scan fallback (`classify_row`)
            // so action resolution (Cascade dominates SetNull) is identical
            // to the scan path — guaranteeing the same rows are affected
            // whether or not an index exists.
            for id in candidate_ids {
                let bytes = match child_table.read_one_tx_bytes(id, Some(tx)).await {
                    Ok(Some(b)) => b,
                    Ok(None) => continue,
                    Err(e) => {
                        return Err(BatchError::QueryError {
                            alias: alias.to_string(),
                            message: format!(
                                "fk_actions: child index fast-path re-read failed: {e}"
                            ),
                            code: Some("fk_actions".to_string()),
                        })
                    }
                };
                classify_row(
                    id,
                    bytes.as_ref(),
                    &resolved_probes,
                    &mut cascade_ids,
                    &mut setnull_ids,
                );
            }
        } else {
            // Fallback: full scan with field match. Tx-aware via
            // `list_stream_tx` (overlays this tx's own staged writes so a
            // staged insert/update/delete on the child table is correctly
            // reflected). `list_stream_tx` also records the Serializable
            // `TableScan` predicate at construction time — the RI-barrier
            // token recorded at ENTRY above is the path-agnostic safety net
            // (exactly as in `child_has_reference`).
            let batch_size = 1000;
            let stream = child_table.list_stream_tx(Some(tx), batch_size);
            futures::pin_mut!(stream);
            while let Some(batch_result) = stream.next().await {
                let batch = batch_result.map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!("fk_actions: child scan failed: {e}"),
                    code: Some("fk_actions".to_string()),
                })?;
                for (id, cow) in batch {
                    let bytes: Bytes = cow_to_bytes(cow).map_err(|e| BatchError::QueryError {
                        alias: alias.to_string(),
                        message: format!("fk_actions: child scan serialize: {e}"),
                        code: Some("fk_actions".to_string()),
                    })?;
                    classify_row(
                        id,
                        bytes.as_ref(),
                        &resolved_probes,
                        &mut cascade_ids,
                        &mut setnull_ids,
                    );
                }
            }
        }

        // ── Record CASCADE deletes ──────────────────────────────────────
        // Dedup by (table, id): a row reachable via two cascade paths
        // (diamond/DAG topology) must be scheduled exactly once.
        for &id in &cascade_ids {
            if dedup.try_delete(&child_name, id) {
                mutations.push(PendingMutation::Delete {
                    table: child_table.clone(),
                    id,
                });
            }
        }

        // ── Record SET NULL updates ─────────────────────────────────────
        for (id, field) in &setnull_ids {
            // Enforce nullable at planning time (action-time check; the
            // engine-mirror schema is not reachable from the DDL bind path).
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
            // Dedup by row identity: skip if already scheduled (deleted or
            // nulled via another cascade path).
            if dedup.try_set_null(&child_name, *id, field) {
                mutations.push(PendingMutation::SetNull {
                    table: child_table.clone(),
                    id: *id,
                    field: field.clone(),
                });
            }
        }

        // ── Recurse for grandchildren (Cascade only) ────────────────────
        // The cascaded child rows may themselves be parents.  Discover their
        // ref_field values and recurse.  We pass the child table as the new
        // parent, with a filter that matches the cascaded row IDs.
        if !cascade_ids.is_empty() {
            // Build a filter that matches the cascaded rows by their IDs.
            // We use the table's primary key — but we don't know its name.
            // Instead, we pass the specific IDs via a dedicated path.
            Box::pin(plan_cascade_for_ids(
                resolver,
                &TableRef::with_repo(&parent_table_ref.repo, &child_name),
                &child_table,
                &cascade_ids,
                alias,
                depth + 1,
                mutations,
                dedup,
                path_guard.visited_mut(),
                tx,
            ))
            .await?;
        }
    }

    Ok(())
}

/// Recurse into grandchildren for a set of specific cascaded child row IDs.
#[allow(clippy::too_many_arguments)]
async fn plan_cascade_for_ids(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
    parent_table: &TableManager,
    parent_ids: &[RecordId],
    alias: &str,
    depth: usize,
    mutations: &mut Vec<PendingMutation>,
    dedup: &mut MutationDedup,
    visited: &mut shamir_collections::TFxSet<String>,
    tx: &shamir_tx::TxContext,
) -> Result<(), BatchError> {
    if depth >= CASCADE_DEPTH_LIMIT {
        return Err(BatchError::query_coded(
            alias,
            "fk_cascade_depth",
            format!(
                "cascade recursion exceeded depth limit ({CASCADE_DEPTH_LIMIT}); \
                 possible FK cycle"
            ),
        ));
    }

    // Per-path cycle guard: see plan_cascade_recursive.
    let mut path_guard = CascadePathGuard::enter(visited, parent_table_ref.table.clone(), alias)?;

    let action_refs = discover_action_refs(resolver, parent_table_ref).await?;
    if action_refs.is_empty() {
        return Ok(());
    }

    // Collect parent ref_field values from the specific cascaded rows.
    let parent_ref_fields: Vec<&str> = {
        let mut fields: Vec<&str> = action_refs
            .iter()
            .map(|r| r.parent_ref_field.as_str())
            .collect();
        fields.sort_unstable();
        fields.dedup();
        fields
    };

    let interner = parent_table
        .interner()
        .get()
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_actions: grandchild interner: {e}"),
            code: Some("fk_actions".to_string()),
        })?;

    // Deduped (`TFxSet`, not `Vec`): several cascaded rows can share the
    // same ref_field value, and a duplicated value would otherwise turn
    // into a duplicated probe fed to `classify_row` for every grandchild row
    // scanned below.
    let mut parent_values: shamir_collections::TFxMap<
        String,
        shamir_collections::TFxSet<QueryValue>,
    > = shamir_collections::TFxMap::default();
    for f in &parent_ref_fields {
        parent_values.insert(f.to_string(), shamir_collections::TFxSet::default());
    }

    // F-28 Step 2: resolve each ref_field's id through the tx-layered
    // interner — `parent_ids` here are CASCADED CHILD rows (this fn recurses
    // into grandchildren), which may themselves have been inserted/updated
    // earlier in this SAME tx with a field name never touched outside it
    // (overlay-only until commit). See `resolve_field_id_layered`'s doc.
    let mut field_ids: shamir_collections::TFxMap<&str, u64> =
        shamir_collections::TFxMap::default();
    for &field in &parent_ref_fields {
        if let Some(id) = resolve_field_id_layered(interner, tx, field).await {
            field_ids.insert(field, id);
        }
    }

    for id in parent_ids {
        let bytes = match parent_table.read_one_tx_bytes(*id, Some(tx)).await {
            Ok(Some(b)) => b,
            Ok(None) => continue,
            Err(e) => {
                return Err(BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!(
                        "fk_actions: grandchild ref_field collection re-read failed: {e}"
                    ),
                    code: Some("fk_actions".to_string()),
                })
            }
        };
        for &field in &parent_ref_fields {
            if let Some(&field_id) = field_ids.get(field) {
                let key = shamir_types::core::interner::InternerKey::new(field_id);
                let path = std::slice::from_ref(&key);
                // Decode-corrupt cascaded row: fail closed instead of
                // silently dropping this row's value from the grandchild
                // discovery set (decode-error twin of the F-65/#891
                // read-error fix — see `extract_ref_field_value`'s doc).
                let qv =
                    extract_ref_field_value(&bytes, path).map_err(|e| BatchError::QueryError {
                        alias: alias.to_string(),
                        message: format!("fk_actions: grandchild ref_field decode failed: {e}"),
                        code: Some("fk_actions".to_string()),
                    })?;
                if let Some(qv) = qv {
                    #[expect(
                        clippy::unwrap_used,
                        reason = "guard: parent_values is initialized for every parent_ref_field"
                    )]
                    parent_values.get_mut(field).unwrap().insert(qv);
                }
            }
        }
    }

    if parent_values.values().all(|v| v.is_empty()) {
        return Ok(());
    }

    // Group refs by child table and resolve handles.
    let mut by_table: shamir_collections::TFxMap<String, Vec<ReverseFkEntry>> =
        shamir_collections::TFxMap::default();
    for rref in action_refs {
        by_table
            .entry(rref.child_table.clone())
            .or_default()
            .push(rref);
    }

    for (child_name, refs) in by_table {
        let has_values = refs.iter().any(|r| {
            parent_values
                .get(r.parent_ref_field.as_str())
                .is_some_and(|v| !v.is_empty())
        });
        if !has_values {
            continue;
        }

        let child_table_ref = TableRef::with_repo(&parent_table_ref.repo, &child_name);
        let child_table =
            resolver
                .resolve(&child_table_ref)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!(
                        "fk_actions: cannot resolve grandchild table '{}': {e}",
                        child_name
                    ),
                    code: Some("fk_actions".to_string()),
                })?;

        // Build probes for this grandchild.
        let mut probes: Vec<(&str, &QueryValue, FkAction)> = Vec::new();
        for r in &refs {
            if let Some(vals) = parent_values.get(r.parent_ref_field.as_str()) {
                for v in vals {
                    probes.push((r.child_field.as_str(), v, r.on_delete));
                }
            }
        }
        if probes.is_empty() {
            continue;
        }

        // Scan grandchild rows.
        let gc_interner =
            child_table
                .interner()
                .get()
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!("fk_actions: grandchild interner: {e}"),
                    code: Some("fk_actions".to_string()),
                })?;

        // F-28 Step 2: resolve each probe's field id through the tx-layered
        // interner — see the sibling comment in `plan_cascade_recursive`.
        let mut resolved_probes: Vec<(u64, &QueryValue, FkAction, &str)> =
            Vec::with_capacity(probes.len());
        for (field, value, action) in &probes {
            if let Some(field_id) = resolve_field_id_layered(gc_interner, tx, field).await {
                resolved_probes.push((field_id, value, *action, field));
            }
        }
        // F-28 Step 5 (memo §2.4): do NOT `continue` on an empty
        // `resolved_probes` — see the identical rationale in
        // `plan_cascade_recursive` above. `list_stream_tx` must run
        // unconditionally so its Serializable `TableScan` predicate is
        // recorded even for a grandchild table whose FK field was never
        // interned; the per-row match loop below is a harmless no-op when
        // `resolved_probes` is empty.

        let mut gc_cascade_ids: Vec<RecordId> = Vec::new();
        let mut gc_setnull_ids: Vec<(RecordId, String)> = Vec::new();

        // F-40b (RI barrier): record this grandchild-table scan at ENTRY,
        // regardless of isolation — same recording as the direct-child scan
        // in `plan_cascade_recursive` above, closing the cross-transaction
        // race for the explicit-Snapshot path at this recursion level too.
        // Recorded BEFORE the index fast-path branch below so it fires
        // regardless of which sub-path executes (mirrors
        // `child_has_reference`'s entry-time ordering exactly).
        tx.record_ri_barrier(child_table.table_token());

        // F-53c: index fast-path — same shape as the direct-child scan in
        // `plan_cascade_recursive` above (see that site's comment for the
        // rationale). See `index_candidate_ids`'s doc for the ordering
        // contract (RI barrier must already be recorded).
        let index_probes: Vec<(&str, &QueryValue)> = resolved_probes
            .iter()
            .map(|(_, value, _, field)| (*field, *value))
            .collect();
        let no_staged_child_writes = tx
            .write_set
            .get(&child_table.table_token())
            .is_none_or(|s| s.is_empty());
        let fast_path = if no_staged_child_writes {
            index_candidate_ids(&child_table, gc_interner, &index_probes)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!("fk_actions: grandchild index lookup failed: {e}"),
                    code: Some("fk_actions".to_string()),
                })?
        } else {
            None
        };

        if let Some(candidate_ids) = fast_path {
            for id in candidate_ids {
                let bytes = match child_table.read_one_tx_bytes(id, Some(tx)).await {
                    Ok(Some(b)) => b,
                    Ok(None) => continue,
                    Err(e) => {
                        return Err(BatchError::QueryError {
                            alias: alias.to_string(),
                            message: format!(
                                "fk_actions: grandchild index fast-path re-read failed: {e}"
                            ),
                            code: Some("fk_actions".to_string()),
                        })
                    }
                };
                classify_row(
                    id,
                    bytes.as_ref(),
                    &resolved_probes,
                    &mut gc_cascade_ids,
                    &mut gc_setnull_ids,
                );
            }
        } else {
            let batch_size = 1000;
            let stream = child_table.list_stream_tx(Some(tx), batch_size);
            futures::pin_mut!(stream);
            while let Some(batch_result) = stream.next().await {
                let batch = batch_result.map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!("fk_actions: grandchild scan failed: {e}"),
                    code: Some("fk_actions".to_string()),
                })?;
                for (id, cow) in batch {
                    let bytes: Bytes = cow_to_bytes(cow).map_err(|e| BatchError::QueryError {
                        alias: alias.to_string(),
                        message: format!("fk_actions: grandchild scan serialize: {e}"),
                        code: Some("fk_actions".to_string()),
                    })?;
                    classify_row(
                        id,
                        bytes.as_ref(),
                        &resolved_probes,
                        &mut gc_cascade_ids,
                        &mut gc_setnull_ids,
                    );
                }
            }
        }

        for &id in &gc_cascade_ids {
            if dedup.try_delete(&child_name, id) {
                mutations.push(PendingMutation::Delete {
                    table: child_table.clone(),
                    id,
                });
            }
        }
        for (id, field) in &gc_setnull_ids {
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
            if dedup.try_set_null(&child_name, *id, field) {
                mutations.push(PendingMutation::SetNull {
                    table: child_table.clone(),
                    id: *id,
                    field: field.clone(),
                });
            }
        }

        // Recurse deeper if there are cascaded grandchildren.
        if !gc_cascade_ids.is_empty() {
            Box::pin(plan_cascade_for_ids(
                resolver,
                &TableRef::with_repo(&parent_table_ref.repo, &child_name),
                &child_table,
                &gc_cascade_ids,
                alias,
                depth + 1,
                mutations,
                dedup,
                path_guard.visited_mut(),
                tx,
            ))
            .await?;
        }
    }

    Ok(())
}
