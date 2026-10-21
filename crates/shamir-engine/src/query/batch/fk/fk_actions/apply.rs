use shamir_query_types::batch::BatchError;
use shamir_types::types::record_id::RecordId;

use crate::table::TableManager;

use super::helpers::null_out_field;
use super::plan::{CascadePlan, PendingMutation};

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

    // #1205: `new_bytes` nulls out an existing field id (not captured at
    // this call site) — leave A8's decode fallback in effect for this
    // table (`ids_captured = false`).
    table
        .update_tx_bytes(id, &old_bytes, new_bytes, tx, false)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("fk_actions: set_null update_tx_bytes failed: {e}"),
            code: Some("fk_actions".to_string()),
        })?;

    Ok(())
}
