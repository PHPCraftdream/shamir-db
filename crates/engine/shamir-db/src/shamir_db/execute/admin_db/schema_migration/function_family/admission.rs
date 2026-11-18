//! Shared read-only admission for server-global function and validator operations.

use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationFunctionDependencies, SchemaMigrationOp,
};
use shamir_query_types::admin::{CheckFunctionOp, CheckFunctionSpec};

use crate::access::{Action, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};

use super::super::super::admin_dispatch::ShamirAdminExecutor;
use super::super::plan::coded;
use super::{execution, registration};

pub(in crate::shamir_db::execute::admin_db_schema_migration) fn family_batch_op(
    operation: &SchemaMigrationOp,
) -> Option<BatchOp> {
    match operation {
        SchemaMigrationOp::CreateFunction(op) => Some(BatchOp::CreateFunction((**op).clone())),
        SchemaMigrationOp::DropFunction(op) => Some(BatchOp::DropFunction((**op).clone())),
        SchemaMigrationOp::RenameFunction(op) => Some(BatchOp::RenameFunction((**op).clone())),
        SchemaMigrationOp::CheckFunction(op) => Some(BatchOp::CheckFunction((**op).clone())),
        SchemaMigrationOp::AlterFunction(op) => Some(BatchOp::AlterFunction((**op).clone())),
        SchemaMigrationOp::CreateValidator(op) => Some(BatchOp::CreateValidator((**op).clone())),
        SchemaMigrationOp::DropValidator(op) => Some(BatchOp::DropValidator((**op).clone())),
        SchemaMigrationOp::RenameValidator(op) => Some(BatchOp::RenameValidator((**op).clone())),
        SchemaMigrationOp::BindValidator(op) => Some(BatchOp::BindValidator((**op).clone())),
        SchemaMigrationOp::UnbindValidator(op) => Some(BatchOp::UnbindValidator((**op).clone())),
        SchemaMigrationOp::CreateTable(_)
        | SchemaMigrationOp::DropTable(_)
        | SchemaMigrationOp::RenameTable(_)
        | SchemaMigrationOp::CreateIndex(_)
        | SchemaMigrationOp::DropIndex(_)
        | SchemaMigrationOp::RenameIndex(_)
        | SchemaMigrationOp::SetTableSchema(_)
        | SchemaMigrationOp::AddSchemaRule(_)
        | SchemaMigrationOp::RemoveSchemaRule(_) => None,
    }
}

pub(in crate::shamir_db::execute::admin_db_schema_migration) fn is_function_family(
    operation: &SchemaMigrationOp,
) -> bool {
    match operation {
        SchemaMigrationOp::CreateFunction(_)
        | SchemaMigrationOp::DropFunction(_)
        | SchemaMigrationOp::RenameFunction(_)
        | SchemaMigrationOp::CheckFunction(_)
        | SchemaMigrationOp::AlterFunction(_)
        | SchemaMigrationOp::CreateValidator(_)
        | SchemaMigrationOp::DropValidator(_)
        | SchemaMigrationOp::RenameValidator(_)
        | SchemaMigrationOp::BindValidator(_)
        | SchemaMigrationOp::UnbindValidator(_) => true,
        SchemaMigrationOp::CreateTable(_)
        | SchemaMigrationOp::DropTable(_)
        | SchemaMigrationOp::RenameTable(_)
        | SchemaMigrationOp::CreateIndex(_)
        | SchemaMigrationOp::DropIndex(_)
        | SchemaMigrationOp::RenameIndex(_)
        | SchemaMigrationOp::SetTableSchema(_)
        | SchemaMigrationOp::AddSchemaRule(_)
        | SchemaMigrationOp::RemoveSchemaRule(_) => false,
    }
}

pub(in crate::shamir_db::execute::admin_db_schema_migration) fn validate_binding_scope(
    db: &str,
    operation: &SchemaMigrationOp,
) -> Result<(), BatchError> {
    let target = match operation {
        SchemaMigrationOp::BindValidator(op) => &op.db,
        SchemaMigrationOp::UnbindValidator(op) => &op.db,
        _ => return Ok(()),
    };
    if target != db {
        return Err(coded(
            "schema_migration_identity_mismatch",
            "validator binding operations must target the fenced database".to_string(),
        ));
    }
    Ok(())
}

/// The batch fence does not drain already-admitted transactions. Validators
/// run at stage time, not commit; even a table write barrier around a scan
/// and bind cannot prevent a previously validated staged write from committing.
pub(in crate::shamir_db::execute::admin_db_schema_migration) fn defer_bind(
    operation: &SchemaMigrationOp,
) -> Result<(), BatchError> {
    if let SchemaMigrationOp::BindValidator(op) = operation {
        return Err(BatchError::QueryError {
            alias: String::new(),
            code: None,
            message: format!(
                "schema_migration_operation_deferred: cannot safely validate existing rows \
                 before binding validator '{}' to table '{}.{}.{}'",
                op.bind_validator, op.db, op.repo, op.table,
            ),
        });
    }
    Ok(())
}

/// Check function creates via the existing dispatch path. Validator candidates
/// use their own handler, not function admission. AlterFunction changes only
/// optimize; its wire DTO carries no code. Explicit checks also run in order
/// in the runner, independently of this read-only pass.
pub(in crate::shamir_db::execute::admin_db_schema_migration) async fn preflight_checks(
    executor: &ShamirAdminExecutor,
    operations: &[SchemaMigrationOp],
) -> Result<(), BatchError> {
    for operation in operations {
        let check_function = match operation {
            SchemaMigrationOp::CreateFunction(op) => CheckFunctionSpec {
                lang: op.lang.clone(),
                wasm: op.wasm.clone(),
                source: op.source.clone(),
                optimize: op.optimize.clone(),
            },
            SchemaMigrationOp::CheckFunction(op) => op.check_function.clone(),
            _ => continue,
        };
        let op = BatchOp::CheckFunction(CheckFunctionOp { check_function });
        Box::pin(execution::execute(executor, &op)).await?;
    }
    Ok(())
}

async fn authorize(
    executor: &ShamirAdminExecutor,
    path: &ResourcePath,
    action: Action,
) -> Result<(), BatchError> {
    Box::pin(
        executor
            .shamir
            .authorize_access(&executor.actor, path, action),
    )
    .await
    .map_err(|_| {
        coded(
            "access_denied",
            "function-family authorization denied".to_string(),
        )
    })
}

/// Mirrors the handler's target checks before fresh execution/replay. The
/// handler must still run its own authorization, including checks on locked
/// catalogue rows. An absent source is not authority to skip that dispatch.
pub(in crate::shamir_db::execute::admin_db_schema_migration) async fn authorize_family_operation(
    executor: &ShamirAdminExecutor,
    operation: &SchemaMigrationOp,
) -> Result<(), BatchError> {
    let (path, action) = match operation {
        SchemaMigrationOp::CreateFunction(_)
        | SchemaMigrationOp::CheckFunction(_)
        | SchemaMigrationOp::CreateValidator(_) => {
            (ResourcePath::FunctionNamespace, Action::Create)
        }
        SchemaMigrationOp::DropFunction(op) => {
            (ResourcePath::function(&op.drop_function), Action::Delete)
        }
        SchemaMigrationOp::RenameFunction(op) => {
            Box::pin(authorize(
                executor,
                &ResourcePath::FunctionNamespace,
                Action::Create,
            ))
            .await?;
            (ResourcePath::function(&op.rename_function), Action::Write)
        }
        SchemaMigrationOp::AlterFunction(op) => {
            (ResourcePath::function(&op.alter_function), Action::Write)
        }
        SchemaMigrationOp::DropValidator(_) => (ResourcePath::FunctionNamespace, Action::Delete),
        SchemaMigrationOp::RenameValidator(_) => (ResourcePath::FunctionNamespace, Action::Write),
        SchemaMigrationOp::BindValidator(op) => (
            ResourcePath::table(op.db.clone(), op.repo.clone(), op.table.clone()),
            Action::Write,
        ),
        SchemaMigrationOp::UnbindValidator(op) => (
            ResourcePath::table(op.db.clone(), op.repo.clone(), op.table.clone()),
            Action::Write,
        ),
        _ => return Ok(()),
    };
    Box::pin(authorize(executor, &path, action)).await
}

/// For recovered function renames only, after journal and registration
/// evidence prove the effect. The destination carries the moved ACL. This
/// must never authorize a replay (which needs the actual source ACL).
pub(in crate::shamir_db::execute::admin_db_schema_migration) async fn authorize_proven_rename(
    executor: &ShamirAdminExecutor,
    operation: &SchemaMigrationOp,
) -> Result<(), BatchError> {
    if let SchemaMigrationOp::RenameFunction(op) = operation {
        Box::pin(authorize(
            executor,
            &ResourcePath::FunctionNamespace,
            Action::Create,
        ))
        .await?;
        Box::pin(authorize(
            executor,
            &ResourcePath::function(&op.to),
            Action::Write,
        ))
        .await
    } else {
        Box::pin(authorize_family_operation(executor, operation)).await
    }
}

/// Both endpoints require read authorization, even when unresolved. Apply
/// refuses unresolved declarations; plan can still display them. Resolution
/// checks current durable and live state, not signatures or call sites.
pub(in crate::shamir_db::execute::admin_db_schema_migration) async fn preflight_dependencies(
    executor: &ShamirAdminExecutor,
    dependencies: &[SchemaMigrationFunctionDependencies],
    require_resolved: bool,
) -> Result<bool, BatchError> {
    let mut all_resolved = true;
    for edge in dependencies {
        for name in std::iter::once(&edge.function).chain(&edge.depends_on) {
            let path = ResourcePath::function(name);
            Box::pin(authorize(executor, &path, Action::Read)).await?;
            if !matches!(
                Box::pin(registration::function(executor, name)).await,
                Ok(Some(_))
            ) {
                all_resolved = false;
                if require_resolved {
                    return Err(coded(
                        "schema_migration_postcondition_failed",
                        format!(
                            "declared function dependency endpoint '{name}' could not be resolved"
                        ),
                    ));
                }
            }
        }
    }
    Ok(all_resolved)
}
