//! Phase D.1 — `ON DELETE RESTRICT` gate for the batch delete path.
//!
//! Before a `BatchOp::Delete` dispatches to `execute_delete_tx`, this gate
//! checks whether any child table has a foreign-key constraint referencing the
//! parent table being deleted from with `on_delete = Restrict`.  If a child
//! row still references one of the parent rows about to be deleted, the
//! operation is rejected with `fk_restrict`.
//!
//! ## TOCTOU caveat
//!
//! The reverse-FK check runs **before** the delete, outside the atomic scope
//! of the transaction that will perform the actual removal.  Between the
//! check and the delete, a concurrent insert into a child table could create
//! a new reference to the about-to-be-deleted parent row.  Tightening this
//! to an in-tx atomic check requires an `Arc<dyn TableResolver>` (so the
//! resolver can be captured into the HRTB closure of
//! `run_implicit_batch_tx`), which is a larger refactor tracked as a future
//! task.  For now, the pre-check is acceptable: delete is not a hot path,
//! Restrict tables are opt-in, and the window is small.

use bytes::Bytes;
use futures::StreamExt;

use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::BatchError;
use shamir_query_types::filter::Filter;
use shamir_types::record_view::{scalar_ref_cmp_qv, RecordRef, RecordView, ScalarRef};
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::batch::TableResolver;
use crate::query::filter::{compile_filter, FilterContext, FilterNode};
use crate::query::TableRef;
use crate::table::record_cow::RecordCow;
use crate::table::TableManager;

/// A single reverse-FK reference: a child table + child field that references
/// the parent table with `on_delete = Restrict`.
struct RestrictRef {
    /// The child table name (same repo as the parent).
    child_table: String,
    /// The child field path that holds the FK value (first segment only for
    /// the child-row probe).
    child_field: String,
    /// The parent field that the child references (`ref_field`).
    parent_ref_field: String,
}

/// Pre-delete RESTRICT gate.
///
/// Scans the repo for child tables whose schema declares a foreign key
/// referencing `parent_table` with `on_delete = Restrict`.  If any child row
/// still references a parent row matched by the delete's `where_clause`, the
/// delete is rejected.
///
/// Returns `Ok(())` to proceed with the delete, or `Err(BatchError)` to reject.
pub(crate) async fn check_fk_restrict(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
    parent_table: &TableManager,
    delete_where: &Filter,
    ctx: &FilterContext<'_>,
    alias: &str,
) -> Result<(), BatchError> {
    // 1. Discover reverse-FK references with Restrict.
    let restrict_refs = discover_restrict_refs(resolver, parent_table_ref).await?;
    if restrict_refs.is_empty() {
        return Ok(());
    }

    // 2. Collect the parent ref_field values from rows about to be deleted.
    let parent_ref_fields: Vec<&str> = {
        let mut fields: Vec<&str> = restrict_refs
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
            message: format!("fk_restrict: parent scan failed: {e}"),
            code: Some("fk_restrict".to_string()),
        })?;

    if parent_values.is_empty() {
        return Ok(());
    }

    // 3. For each restrict ref, check if any child row references one of the
    //    parent values.
    for rref in &restrict_refs {
        let values_for_field = match parent_values.get(rref.parent_ref_field.as_str()) {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };

        let child_table_ref = TableRef::with_repo(&parent_table_ref.repo, &rref.child_table);
        let child_table =
            resolver
                .resolve(&child_table_ref)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!(
                        "fk_restrict: cannot resolve child table '{}': {e}",
                        rref.child_table
                    ),
                    code: Some("fk_restrict".to_string()),
                })?;

        for parent_val in values_for_field {
            let exists = child_has_reference(&child_table, &rref.child_field, parent_val)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!("fk_restrict: child scan failed: {e}"),
                    code: Some("fk_restrict".to_string()),
                })?;

            if exists {
                return Err(BatchError::query_coded(
                    alias,
                    "fk_restrict",
                    format!(
                        "cannot delete from '{}': row is still referenced by '{}.{}'",
                        parent_table_ref.table, rref.child_table, rref.child_field,
                    ),
                ));
            }
        }
    }

    Ok(())
}

/// Discover all child tables in the repo that have a FK pointing at
/// `parent_table` with `on_delete = Restrict`.
async fn discover_restrict_refs(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
) -> Result<Vec<RestrictRef>, BatchError> {
    let repo = resolver
        .resolve_repo(&parent_table_ref.repo)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("fk_restrict: resolve_repo({}): {e}", parent_table_ref.repo),
            code: Some("fk_restrict".to_string()),
        })?;

    let table_names = repo.list_table_names();
    let mut refs = Vec::new();

    for name in &table_names {
        let child_ref = TableRef::with_repo(&parent_table_ref.repo, name);
        let child_table = match resolver.resolve(&child_ref).await {
            Ok(t) => t,
            Err(_) => continue,
        };

        for (field_path, fk) in child_table.collect_fk_refs() {
            if fk.ref_table == parent_table_ref.table && fk.on_delete == FkAction::Restrict {
                // Use the first segment of the field path as the child field
                // name (single-segment FK fields are the common case).
                let child_field = field_path.join(".");
                refs.push(RestrictRef {
                    child_table: name.clone(),
                    child_field,
                    parent_ref_field: fk.ref_field.clone(),
                });
            }
        }
    }

    Ok(refs)
}

/// Scan the parent table for rows matching `where_clause` and extract the
/// values of the given `ref_fields`.
///
/// Returns a map: `field_name -> Vec<QueryValue>` (the set of distinct values
/// from the matched rows).
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

            // Extract the ref_field values from the matched row.
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

/// Check whether any row in `child_table` has `field == value`.
///
/// Mirrors `ValidatorDb::exists_in_table` semantics: index lookup if covered,
/// else full scan with field match.
async fn child_has_reference(
    table: &TableManager,
    field: &str,
    value: &QueryValue,
) -> shamir_storage::error::DbResult<bool> {
    let interner = table.interner().get().await?;

    // Fast path: single-field index lookup.
    if let Some(inner_value) = qv_scalar_to_inner(value) {
        if let Some(field_id) = interner.get_ind(field) {
            let field_path = [field_id.id()];
            if let Some(idx_name) = table.find_single_field_index(&field_path) {
                let ids = table
                    .index_manager_ref()
                    .lookup_by_index(idx_name, std::slice::from_ref(&inner_value))
                    .await?;
                return Ok(!ids.is_empty());
            }
        }
    }

    // Fallback: full scan with field match.
    let batch_size = 1000;
    let stream = table.list_stream(batch_size);
    futures::pin_mut!(stream);
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        for (_, cow) in batch {
            let bytes: Bytes = cow_to_bytes(cow)?;
            if record_field_matches(bytes.as_ref(), field, value, interner) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn cow_to_bytes(cow: RecordCow) -> shamir_storage::error::DbResult<Bytes> {
    match cow {
        RecordCow::Borrowed(b) => Ok(b),
        RecordCow::Owned(tree) => tree.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("fk_restrict scan serialize: {e}"))
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

fn record_field_matches(
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
    // Delegate to the cross-type-comparing `scalar_ref_cmp_qv` so that a parent
    // key stored as `Int(5)` matches a child FK field stored as `F64(5.0)` (and
    // vice-versa) — consistent with every other comparison layer in the engine.
    scalar_ref_cmp_qv(*actual, value) == Some(std::cmp::Ordering::Equal)
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
