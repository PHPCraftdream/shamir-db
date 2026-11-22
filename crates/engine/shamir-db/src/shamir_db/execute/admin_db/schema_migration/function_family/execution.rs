//! Direct dispatch of one function-family operation to its existing admin handler.
//!
//! The generic `execute_admin` dispatcher future is the union of every admin
//! handler's state: building it on the stack below the apply runner's frames
//! overflows a debug test thread. Each operation therefore calls only its own
//! handler, boxed, exactly like the table/index operations do.
//!
//! `RenameValidator` proves gate ownership with the step-10 preflight
//! capability (`handle_rename_validator(op, proof)`); every other
//! operation keeps the unchanged `execute` entry.

use crate::query::batch::{BatchError, BatchOp};

use crate::shamir_db::shamir_db::ValidatorRenameMigration;

use super::super::super::admin_dispatch::ShamirAdminExecutor;

pub(in crate::shamir_db::execute::admin_db_schema_migration) async fn execute(
    executor: &ShamirAdminExecutor,
    op: &BatchOp,
) -> Result<(), BatchError> {
    execute_with_rename(executor, op, None).await
}

/// [`execute`] with the migration's validator-rename capability: `Some`
/// only for a `RenameValidator` effect of a migration whose step-10
/// preflight holds [`ValidatorRenameMigration`]. A `RenameValidator` op
/// without it is a plain typed invariant error, never an unchecked run.
pub(in crate::shamir_db::execute::admin_db_schema_migration) async fn execute_with_rename(
    executor: &ShamirAdminExecutor,
    op: &BatchOp,
    rename: Option<&ValidatorRenameMigration<'_>>,
) -> Result<(), BatchError> {
    match op {
        BatchOp::CreateFunction(_) => Box::pin(executor.handle_create_function(op)).await,
        BatchOp::DropFunction(_) => Box::pin(executor.handle_drop_function(op)).await,
        BatchOp::RenameFunction(_) => Box::pin(executor.handle_rename_function(op)).await,
        BatchOp::CheckFunction(_) => Box::pin(executor.handle_check_function(op)).await,
        BatchOp::AlterFunction(_) => Box::pin(executor.handle_alter_function(op)).await,
        BatchOp::CreateValidator(_) => Box::pin(executor.handle_create_validator(op)).await,
        BatchOp::DropValidator(_) => Box::pin(executor.handle_drop_validator(op)).await,
        BatchOp::RenameValidator(_) => {
            // Runner context is `Option`, but the proof itself is mandatory:
            // never fall through to an unchecked rename admission.
            let Some(proof) = rename else {
                return Err(BatchError::QueryError {
                    alias: String::new(),
                    message: "RenameValidator requires the migration's validator-rename \
                              capability"
                        .to_string(),
                    code: None,
                });
            };
            Box::pin(executor.handle_rename_validator(op, Some(proof))).await
        }
        BatchOp::BindValidator(_) => Box::pin(executor.handle_bind_validator(op)).await,
        BatchOp::UnbindValidator(_) => Box::pin(executor.handle_unbind_validator(op)).await,
        _ => {
            return Err(BatchError::QueryError {
                alias: String::new(),
                message: "unsupported function-family operation".to_string(),
                code: None,
            })
        }
    }
    .map(|_| ())
}
