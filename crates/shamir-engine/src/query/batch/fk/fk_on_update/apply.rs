use bytes::Bytes;

use shamir_query_types::batch::BatchError;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::table::TableManager;

use super::helpers::qv_to_inner_scalar;
use super::plan::{FkUpdatePlan, PendingMutation};

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

pub(super) async fn field_is_nullable(table: &TableManager, field: &str) -> bool {
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
