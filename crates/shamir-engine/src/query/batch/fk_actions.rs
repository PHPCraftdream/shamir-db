//! Phase D.2 — `ON DELETE CASCADE` + `ON DELETE SET NULL`.
//!
//! Extends the Phase D.1 reverse-FK discovery + gate machinery to the
//! remaining referential actions.  Where D.1 (`RESTRICT`) only *rejects* the
//! parent delete, D.2 *acts* on the child rows:
//!
//! - **Cascade** — for each child row whose FK field equals a parent value
//!   being deleted, DELETE the child row.  Recurses: a cascaded child may
//!   itself be a parent of grandchildren.  A depth guard prevents infinite
//!   loops on FK cycles (returns `fk_cascade_depth`).
//! - **SetNull** — for each child row, UPDATE it setting the FK field to
//!   `Null`.  The child field must be nullable; this is checked at action
//!   time (`set_null_requires_nullable`) because the engine-mirror schema
//!   lives on the `TableManager` and is not reachable from the DDL bind path.
//!
//! ## Atomicity
//!
//! The cascade/setnull mutations run **inside the same implicit/tx batch** as
//! the parent delete so they commit atomically.  The full cascade tree
//! (including grandchild discovery) is now resolved **against the calling
//! transaction's own snapshot-plus-staged-writes view** (F-28 Step 2), via
//! `TableManager::list_stream_tx(Some(tx), ..)` and
//! `read_one_tx_bytes(id, Some(tx))` — planning still happens before the
//! actual mutation calls (`delete_tx` / `update_tx_bytes`) but now reads
//! read-your-own-writes state rather than committed-only state.
//!
//! ## TOCTOU caveat
//!
//! F-28 Step 2 closes the **in-tx** read-your-own-writes gap (D1): a child
//! row inserted earlier in the SAME transaction (before the parent delete
//! that triggers this cascade) is now visible to `collect_parent_values` /
//! `discover_action_refs`'s row-level scans, so it is correctly included in
//! the cascade/setnull plan instead of silently orphaned. A child row staged
//! as an UPDATE re-keying its FK away from the parent is likewise reflected
//! via the tx overlay, not the stale committed value.
//!
//! ## Cross-transaction race — CLOSED (F-28 Step 5, S3-C)
//!
//! The **cross-transaction** race — a genuinely concurrent OTHER
//! transaction inserting a new child reference between this plan and the
//! eventual commit — is now closed the same way `fk_restrict.rs` closes it:
//! the parent-side implicit-delete isolation upgrade
//! (`query_runner.rs`'s `implicit_tx_isolation_for_fk_parent`, gated on
//! `FkReverseCache::is_fk_parent_with_delete_action`) plus the child-side footprint
//! widening (`require_footprint_if_fk_child`, gated on
//! `FkReverseCache::is_fk_child`) — see `fk_restrict.rs`'s module doc for
//! the full mechanism description and
//! `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs`
//! for the deterministic end-to-end proof (a CASCADE-shaped race is
//! action-agnostic to this mechanism — it protects any Serializable scan of
//! the child table regardless of what the caller does with the result).
//!
//! Residual scope: same as `fk_restrict.rs` — closed for the **implicit**
//! delete path; an explicit tx that stays Snapshot is unaffected unless the
//! caller itself opens it Serializable.

use bytes::Bytes;
use futures::StreamExt;

use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::BatchError;
use shamir_query_types::filter::Filter;
use shamir_types::record_view::{scalar_ref_cmp_qv, RecordRef, RecordView, ScalarRef};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::batch::TableResolver;
use crate::query::filter::{compile_filter, FilterContext, FilterNode};
use crate::query::TableRef;
use crate::repo::{build_reverse_fk_entries, ReverseFkEntry};
use crate::table::record_cow::RecordCow;
use crate::table::TableManager;

/// Maximum cascade recursion depth before we reject with `fk_cascade_depth`.
///
/// This is a safety valve against FK cycles (A→B→A).  Real cascade chains are
/// shallow (typically 2–3 levels); 32 is generous while still bounding the
/// worst case.
pub(crate) const CASCADE_DEPTH_LIMIT: usize = 32;

/// A single mutation to apply inside the tx.
#[derive(Clone)]
enum PendingMutation {
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
    mutations: Vec<PendingMutation>,
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
/// inside the tx via [`apply_cascade_plan`].
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
                    _ => continue,
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

    let mut parent_values: shamir_collections::TFxMap<String, Vec<QueryValue>> =
        shamir_collections::TFxMap::default();
    for f in &parent_ref_fields {
        parent_values.insert(f.to_string(), Vec::new());
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
            _ => continue,
        };
        for &field in &parent_ref_fields {
            if let Some(&field_id) = field_ids.get(field) {
                let key = shamir_types::core::interner::InternerKey::new(field_id);
                let path = std::slice::from_ref(&key);
                if let Ok(view) = RecordView::new(&bytes) {
                    if let Some(scalar) = view.scalar_at(path) {
                        let qv = scalar_ref_to_qv(scalar);
                        parent_values.get_mut(field).unwrap().push(qv);
                    }
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
                    _ => continue,
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

/// Apply the cascade/setnull plan inside the tx.
///
/// Executes all pending mutations (deletes + setnull updates) in order.
/// No resolver needed — the plan carries pre-resolved `TableManager` handles.
pub(crate) async fn apply_cascade_plan(
    plan: CascadePlan,
    tx: &mut shamir_tx::TxContext,
    alias: &str,
) -> Result<(), BatchError> {
    if plan.is_empty() {
        return Ok(());
    }

    for mutation in plan.mutations {
        match mutation {
            PendingMutation::Delete { table, id } => {
                table
                    .delete_tx(id, Some(tx))
                    .await
                    .map_err(|e| BatchError::QueryError {
                        alias: alias.to_string(),
                        message: format!("fk_actions: cascade delete_tx failed: {e}"),
                        code: Some("fk_actions".to_string()),
                    })?;
            }
            PendingMutation::SetNull { table, id, field } => {
                set_child_field_null(&table, id, &field, tx, alias).await?;
            }
        }
    }

    Ok(())
}

/// Set a child row's field to Null via an in-tx update.
///
/// Reads the current row bytes, replaces the field value with Null, and stages
/// the update via `update_tx_bytes` (the byte-level merge path).
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
            message: format!("fk_actions: set_null read failed: {e}"),
            code: Some("fk_actions".to_string()),
        })?
        .ok_or_else(|| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_actions: set_null row {:?} not found", id),
            code: Some("fk_actions".to_string()),
        })?;

    let interner = table
        .interner()
        .get()
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_actions: set_null interner: {e}"),
            code: Some("fk_actions".to_string()),
        })?;

    let field_id = match interner.get_ind(field) {
        Some(id) => id,
        None => {
            return Err(BatchError::query_coded(
                alias,
                "fk_actions",
                format!("set_null: unknown field '{}'", field),
            ));
        }
    };

    let new_bytes =
        null_out_field(&old_bytes, field_id.id()).map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_actions: set_null merge failed: {e}"),
            code: Some("fk_actions".to_string()),
        })?;

    table
        .update_tx_bytes(id, &old_bytes, new_bytes, tx)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_actions: set_null update_tx_bytes failed: {e}"),
            code: Some("fk_actions".to_string()),
        })?;

    Ok(())
}

/// Check whether a field is nullable per the table's schema validators.
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

/// Produce a new storage-bytes map with `field_id` set to Null.
fn null_out_field(
    old_bytes: &Bytes,
    field_id: u64,
) -> Result<Bytes, shamir_storage::error::DbError> {
    let mut tree = InnerValue::from_bytes(old_bytes.clone()).map_err(|e| {
        shamir_storage::error::DbError::Codec(format!("fk_actions null_out_field decode: {e}"))
    })?;
    if let InnerValue::Map(ref mut m) = tree {
        let key = shamir_types::core::interner::InternerKey::new(field_id);
        m.insert(key, InnerValue::Null);
    }
    tree.to_bytes().map_err(|e| {
        shamir_storage::error::DbError::Codec(format!("fk_actions null_out_field encode: {e}"))
    })
}

// ── Discovery ────────────────────────────────────────────────────────────────

/// Discover all child tables with Cascade or SetNull FKs pointing at
/// `parent_table`.
///
/// F-28 Step 4 (#831): reads through the repo's cached reverse-FK map
/// (`RepoInstance::fk_reverse_cache`) instead of re-running the O(tables)
/// `collect_fk_refs()` scan on every call/cascade-recursion-level — a cache
/// miss (never built, or invalidated by a schema mutation since the last
/// build) pays that scan exactly once and seeds every parent table's entry
/// in the repo, not just this one. Shares the SAME cache (and the same
/// underlying scan) `fk_restrict.rs`'s `discover_restrict_refs` populates —
/// the cache carries every `on_delete` action; each caller filters what it
/// needs.
async fn discover_action_refs(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
) -> Result<Vec<ReverseFkEntry>, BatchError> {
    let repo = resolver
        .resolve_repo(&parent_table_ref.repo)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("fk_actions: resolve_repo({}): {e}", parent_table_ref.repo),
            code: Some("fk_actions".to_string()),
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
            message: format!("fk_actions: reverse-FK scan failed: {e}"),
            code: Some("fk_actions".to_string()),
        })?;

    let is_self_ref_table = &parent_table_ref.table;
    Ok(all_for_parent
        .into_iter()
        .filter(|e| {
            if !(e.on_delete == FkAction::Cascade || e.on_delete == FkAction::SetNull) {
                return false;
            }
            // Self-referential CASCADE is not supported by the table-name
            // cascade cycle-guard (it would false-positive as a cycle even
            // for the shallowest self-ref case); it is rejected at DDL time
            // (`validate_no_self_referential_cascade`) and skipped here as
            // defense-in-depth for any pre-existing schema predating that
            // check. Self-referential SET NULL is safe — it is a
            // single-level mutation that never recurses (see the
            // "Recurse for grandchildren (Cascade only)" gate below).
            let is_self_ref = &e.child_table == is_self_ref_table;
            !(is_self_ref && e.on_delete == FkAction::Cascade)
        })
        .collect())
}

// ── Scan helpers (mirrors fk_restrict.rs) ────────────────────────────────────

/// Scan the parent table for rows matching `where_clause` and extract the
/// values of the given `ref_fields`.
async fn collect_parent_values(
    table: &TableManager,
    where_clause: &Filter,
    ctx: &FilterContext<'_>,
    ref_fields: &[&str],
    tx: &shamir_tx::TxContext,
) -> shamir_storage::error::DbResult<shamir_collections::TFxMap<String, Vec<QueryValue>>> {
    let interner = table.interner().get().await?;
    let callback = compile_filter(where_clause, interner);
    let batch_size = 1000;

    let mut result: shamir_collections::TFxMap<String, Vec<QueryValue>> =
        shamir_collections::TFxMap::default();
    for &field in ref_fields {
        result.insert(field.to_string(), Vec::new());
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

    // F-28 Step 2: tx-aware scan — overlays this tx's own staged writes so a
    // parent row deleted/updated EARLIER in the same tx is correctly hidden/
    // reflected (read-your-own-writes), closing D1 for this probe.
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
                    if let Ok(view) = RecordView::new(&bytes) {
                        if let Some(scalar) = view.scalar_at(path) {
                            let qv = scalar_ref_to_qv(scalar);
                            result.get_mut(field).unwrap().push(qv);
                        }
                    }
                }
            }
        }
    }

    Ok(result)
}

fn cow_to_bytes(cow: RecordCow) -> shamir_storage::error::DbResult<Bytes> {
    match cow {
        RecordCow::Borrowed(b) => Ok(b),
        RecordCow::Owned(tree) => tree.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("fk_actions scan serialize: {e}"))
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
    // vice-versa) — consistent with every other comparison layer in the engine
    // (`scalar_ref_cmp_qv`, `compare_values`, the coercing-set probes).
    scalar_ref_cmp_qv(*actual, value) == Some(std::cmp::Ordering::Equal)
}

/// Like `record_field_matches_qv`, but keyed by an already-resolved
/// `InternerKey` instead of a field name. Used when matching against rows
/// that may be tx-overlay-sourced (`list_stream_tx`), where the field id must
/// be resolved via [`resolve_field_id_layered`] rather than a plain base-only
/// `interner.get_ind` (a field name first interned earlier in the SAME tx
/// only exists in `tx.interner_overlay`, not yet in base).
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

/// F-53c: per-row action resolution shared by the index fast-path and the
/// `list_stream_tx` scan fallback in `plan_cascade_recursive` and
/// `plan_cascade_for_ids`. Mutates `cascade_ids` / `setnull_ids` exactly as
/// the original inline loop did: Cascade dominates SetNull (a deleted row
/// can't be nulled). Extracting this into a shared helper guarantees the
/// two paths produce byte-for-byte identical results whether or not a
/// supporting index exists on the FK column.
fn classify_row(
    id: RecordId,
    bytes: &[u8],
    resolved_probes: &[(u64, &QueryValue, FkAction, &str)],
    cascade_ids: &mut Vec<RecordId>,
    setnull_ids: &mut Vec<(RecordId, String)>,
) {
    // Check each probe against this row.  Cascade dominates SetNull
    // (a deleted row can't be nulled).
    let mut row_action: Option<FkAction> = None;
    let mut row_setnull_field: Option<String> = None;
    for (field_id, value, action, field_name) in resolved_probes {
        let key = shamir_types::core::interner::InternerKey::new(*field_id);
        if record_field_matches_by_id(bytes, &key, value) {
            match action {
                FkAction::Cascade => {
                    row_action = Some(FkAction::Cascade);
                    break;
                }
                FkAction::SetNull => {
                    if row_action.is_none() {
                        row_action = Some(FkAction::SetNull);
                        row_setnull_field = Some((*field_name).to_string());
                    }
                }
                _ => {}
            }
        }
    }
    match row_action {
        Some(FkAction::Cascade) => cascade_ids.push(id),
        Some(FkAction::SetNull) => {
            if let Some(f) = row_setnull_field {
                setnull_ids.push((id, f));
            }
        }
        _ => {}
    }
}

/// F-53c: index fast-path candidate gathering, mirroring
/// `fk_restrict::child_has_reference`'s index lookup exactly.
///
/// For each `(field, value)` probe, resolves `field` to its BASE interner id,
/// checks for a single-field index on it (via `find_single_field_index`, the
/// SAME check `child_has_reference` uses for RESTRICT), and looks up the
/// committed RecordIds whose indexed value equals `value`. Returns the UNION
/// of all probes' candidate IDs (deduped) when EVERY probe is index-covered,
/// or `None` as soon as any probe's field lacks a usable single-field index
/// (or any value is `Null` / a non-indexable scalar) — signaling the caller
/// to fall back to the tx-aware `list_stream_tx` scan. `Null` is deliberately
/// treated as non-indexable so the fast-path and scan-fallback produce
/// identical results even for a (theoretically) nullable referenced field.
///
/// The returned IDs reflect COMMITTED state only — the index is maintained
/// at commit, never mid-tx. The caller MUST additionally gate on "no staged
/// writes to this child table" before treating the set as complete; staged
/// inserts/updates/deletes are invisible to the index and are correctly
/// handled only by the scan fallback. The caller MUST have already recorded
/// the RI-barrier token at entry (before this call), preserving the F-40b
/// mutual-serialization ordering exactly as `child_has_reference` does.
async fn index_candidate_ids(
    table: &TableManager,
    interner: &shamir_types::core::interner::Interner,
    probes: &[(&str, &QueryValue)],
) -> shamir_storage::error::DbResult<Option<Vec<RecordId>>> {
    use std::collections::BTreeSet;
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
        let ids = table
            .index_manager_ref()
            .lookup_by_index(idx_name, std::slice::from_ref(&inner))
            .await?;
        all.extend(ids.iter().copied());
    }
    Ok(Some(all.into_iter().collect()))
}

/// F-53c: index fast-path helper — converts a probe's `QueryValue` to its
/// indexable `InnerValue` form. Mirrors the identical helper in
/// `fk_restrict.rs` / `fk_on_update.rs`.
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

/// F-28 Step 2: resolve `field` to its interner id, checking `base` first
/// and then this tx's `interner_overlay` — mirrors
/// `ValidatorDb::exists_in_self`'s staged-writes step (and
/// `shamir_tx::LayeredInterner::get_id`, reimplemented inline here since
/// `LayeredInterner` borrows `&TxContext` fields directly and this call site
/// only has `tx: &shamir_tx::TxContext` on hand, not a pre-built wrapper).
///
/// A child/grandchild row inserted or updated EARLIER IN THE SAME tx may use
/// a field name that has never been touched outside this tx — such a name is
/// allocated only in `tx.interner_overlay`, never in `base`, until commit
/// merges it. A base-only lookup would silently return `None` for it even
/// though `list_stream_tx` already made the staged row itself visible,
/// causing every probe against that field to falsely report "no match".
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
