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
//! (including grandchild discovery) is resolved at PLANNING time (before the
//! tx closure) because the `resolver` cannot be captured into the HRTB
//! closure of `run_implicit_batch_tx` — same constraint documented in D.1's
//! TOCTOU note.  The actual row mutations (`delete_tx` / `update_tx_bytes`)
//! run inside the closure against the pre-resolved handles.
//!
//! ## TOCTOU caveat
//!
//! Same as D.1: between the pre-tx discovery and the tx commit, a concurrent
//! insert into a child table could create a new reference to the
//! about-to-be-deleted parent row.  Tightening to an in-tx atomic check
//! requires `Arc<dyn TableResolver>` (future refactor).  Acceptable for MVP:
//! delete is not a hot path, FK-action tables are opt-in, and the window is
//! small.

use bytes::Bytes;
use futures::StreamExt;

use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::BatchError;
use shamir_query_types::filter::Filter;
use shamir_types::record_view::{RecordRef, RecordView, ScalarRef};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::batch::TableResolver;
use crate::query::filter::{compile_filter, FilterContext, FilterNode};
use crate::query::TableRef;
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

/// Discover + resolve the full cascade/setnull plan.
///
/// Walks the cascade tree recursively (up to [`CASCADE_DEPTH_LIMIT`]) at
/// planning time, collecting all child/grandchild rows that need to be
/// deleted or nulled.  Returns a flat [`CascadePlan`] that the caller drives
/// inside the tx via [`apply_cascade_plan`].
pub(crate) async fn plan_cascade(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
    parent_table: &TableManager,
    delete_where: &Filter,
    ctx: &FilterContext<'_>,
    alias: &str,
) -> Result<CascadePlan, BatchError> {
    let mut mutations = Vec::new();
    let mut visited: shamir_collections::TFxSet<String> = shamir_collections::TFxSet::default();

    plan_cascade_recursive(
        resolver,
        parent_table_ref,
        parent_table,
        delete_where,
        ctx,
        alias,
        0,
        &mut mutations,
        &mut visited,
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
    visited: &mut shamir_collections::TFxSet<String>,
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

    // Cycle guard: track tables we've already cascaded through.  A table
    // appearing twice means there's a FK cycle (e.g. X→Y→X); we stop to avoid
    // infinite recursion.  The depth limit is a secondary safety valve.
    if !visited.insert(parent_table_ref.table.clone()) {
        // Already cascaded through this table — FK cycle detected.
        return Err(BatchError::query_coded(
            alias,
            "fk_cascade_depth",
            format!(
                "cascade cycle detected at table '{}'; \
                 possible FK cycle",
                parent_table_ref.table
            ),
        ));
    }

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

    let parent_values = collect_parent_values(parent_table, delete_where, ctx, &parent_ref_fields)
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
    let mut by_table: shamir_collections::TFxMap<String, Vec<DiscoveredRef>> =
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
                    probes.push((r.child_field.as_str(), v, r.action));
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

        let batch_size = 1000;
        let mut cascade_ids: Vec<RecordId> = Vec::new();
        let mut setnull_ids: Vec<(RecordId, String)> = Vec::new();

        let stream = child_table.list_stream(batch_size);
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

                // Check each probe against this row.  Cascade dominates SetNull
                // (a deleted row can't be nulled).
                let mut row_action: Option<FkAction> = None;
                let mut row_setnull_field: Option<String> = None;
                for (field, value, action) in &probes {
                    if record_field_matches_qv(bytes.as_ref(), field, value, interner) {
                        match action {
                            FkAction::Cascade => {
                                row_action = Some(FkAction::Cascade);
                                break;
                            }
                            FkAction::SetNull => {
                                if row_action.is_none() {
                                    row_action = Some(FkAction::SetNull);
                                    row_setnull_field = Some(field.to_string());
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
        }

        // ── Record CASCADE deletes ──────────────────────────────────────
        for &id in &cascade_ids {
            mutations.push(PendingMutation::Delete {
                table: child_table.clone(),
                id,
            });
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
            mutations.push(PendingMutation::SetNull {
                table: child_table.clone(),
                id: *id,
                field: field.clone(),
            });
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
                visited,
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
    visited: &mut shamir_collections::TFxSet<String>,
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

    // Cycle guard: same as plan_cascade_recursive.
    if !visited.insert(parent_table_ref.table.clone()) {
        return Err(BatchError::query_coded(
            alias,
            "fk_cascade_depth",
            format!(
                "cascade cycle detected at table '{}'; \
                 possible FK cycle",
                parent_table_ref.table
            ),
        ));
    }

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

    for id in parent_ids {
        let bytes = match parent_table.read_one_tx_bytes(*id, None).await {
            Ok(Some(b)) => b,
            _ => continue,
        };
        for &field in &parent_ref_fields {
            if let Some(field_id) = interner.get_ind(field) {
                let path = std::slice::from_ref(&field_id);
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
    let mut by_table: shamir_collections::TFxMap<String, Vec<DiscoveredRef>> =
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
                    probes.push((r.child_field.as_str(), v, r.action));
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

        let batch_size = 1000;
        let mut gc_cascade_ids: Vec<RecordId> = Vec::new();
        let mut gc_setnull_ids: Vec<(RecordId, String)> = Vec::new();

        let stream = child_table.list_stream(batch_size);
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
                let mut row_action: Option<FkAction> = None;
                let mut row_setnull_field: Option<String> = None;
                for (field, value, action) in &probes {
                    if record_field_matches_qv(bytes.as_ref(), field, value, gc_interner) {
                        match action {
                            FkAction::Cascade => {
                                row_action = Some(FkAction::Cascade);
                                break;
                            }
                            FkAction::SetNull => {
                                if row_action.is_none() {
                                    row_action = Some(FkAction::SetNull);
                                    row_setnull_field = Some(field.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                match row_action {
                    Some(FkAction::Cascade) => gc_cascade_ids.push(id),
                    Some(FkAction::SetNull) => {
                        if let Some(f) = row_setnull_field {
                            gc_setnull_ids.push((id, f));
                        }
                    }
                    _ => {}
                }
            }
        }

        for &id in &gc_cascade_ids {
            mutations.push(PendingMutation::Delete {
                table: child_table.clone(),
                id,
            });
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
            mutations.push(PendingMutation::SetNull {
                table: child_table.clone(),
                id: *id,
                field: field.clone(),
            });
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
                visited,
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

/// A reverse-FK reference with its child table name, for discovery.
struct DiscoveredRef {
    child_table: String,
    child_field: String,
    parent_ref_field: String,
    action: FkAction,
}

/// Discover all child tables with Cascade or SetNull FKs pointing at
/// `parent_table`.
async fn discover_action_refs(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
) -> Result<Vec<DiscoveredRef>, BatchError> {
    let repo = resolver
        .resolve_repo(&parent_table_ref.repo)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("fk_actions: resolve_repo({}): {e}", parent_table_ref.repo),
            code: Some("fk_actions".to_string()),
        })?;

    let table_names = repo.list_table_names();
    let mut refs = Vec::new();

    for name in &table_names {
        // Skip the parent table itself — self-referential FKs would cause
        // infinite recursion in cascade.
        if name == &parent_table_ref.table {
            continue;
        }
        let child_ref = TableRef::with_repo(&parent_table_ref.repo, name);
        let child_table = match resolver.resolve(&child_ref).await {
            Ok(t) => t,
            Err(_) => continue,
        };

        for (field_path, fk) in child_table.collect_fk_refs() {
            if fk.ref_table != parent_table_ref.table {
                continue;
            }
            let action = fk.on_delete;
            if action == FkAction::Cascade || action == FkAction::SetNull {
                refs.push(DiscoveredRef {
                    child_table: name.clone(),
                    child_field: field_path.join("."),
                    parent_ref_field: fk.ref_field.clone(),
                    action,
                });
            }
        }
    }

    Ok(refs)
}

// ── Scan helpers (mirrors fk_restrict.rs) ────────────────────────────────────

/// Scan the parent table for rows matching `where_clause` and extract the
/// values of the given `ref_fields`.
async fn collect_parent_values(
    table: &TableManager,
    where_clause: &Filter,
    ctx: &FilterContext<'_>,
    ref_fields: &[&str],
) -> shamir_storage::error::DbResult<shamir_collections::TFxMap<String, Vec<QueryValue>>> {
    let interner = table.interner().get().await?;
    let callback = compile_filter(where_clause, interner);
    let batch_size = 1000;

    let mut result: shamir_collections::TFxMap<String, Vec<QueryValue>> =
        shamir_collections::TFxMap::default();
    for &field in ref_fields {
        result.insert(field.to_string(), Vec::new());
    }

    let stream = table.list_stream(batch_size);
    futures::pin_mut!(stream);
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        for (_id, cow) in batch {
            let bytes: Bytes = cow_to_bytes(cow)?;

            if !filter_matches(&bytes, &callback, ctx) {
                continue;
            }

            for &field in ref_fields {
                if let Some(field_id) = interner.get_ind(field) {
                    let path = std::slice::from_ref(&field_id);
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

fn record_field_matches_qv(
    record_bytes: &[u8],
    field: &str,
    value: &QueryValue,
    interner: &shamir_types::core::interner::Interner,
) -> bool {
    if let Some(field_id) = interner.get_ind(field) {
        let path = std::slice::from_ref(&field_id);
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
    }
    false
}

fn scalar_ref_matches_qv(actual: &ScalarRef<'_>, value: &QueryValue) -> bool {
    match (actual, value) {
        (ScalarRef::Null, QueryValue::Null) => true,
        (ScalarRef::Bool(a), QueryValue::Bool(b)) => a == b,
        (ScalarRef::Int(a), QueryValue::Int(b)) => a == b,
        (ScalarRef::F64(a), QueryValue::F64(b)) => a == b,
        (ScalarRef::Str(a), QueryValue::Str(b)) => *a == b.as_str(),
        (ScalarRef::Bin(a), QueryValue::Bin(b)) => *a == b.as_slice(),
        _ => false,
    }
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
