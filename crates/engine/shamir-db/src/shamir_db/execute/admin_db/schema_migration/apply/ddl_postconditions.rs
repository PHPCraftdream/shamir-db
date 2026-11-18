//! Exact table/index/schema-rule postconditions shared by apply and reconcile.

use shamir_query_types::admin::types::schema_migration::SchemaMigrationOp;

use crate::query::admin::FieldRuleDto;
use crate::query::batch::BatchError;
use crate::shamir_db::shamir_db::schema_management::SCHEMA_FIELD;
use crate::shamir_db::ShamirDb;

use super::super::super::super::admin_dispatch::ShamirAdminExecutor;
use super::super::super::super::schema_codec::checked_catalogue_rules;
use super::super::super::plan::{coded, map_store_error};
use super::super::apply_journal::plain;

pub(super) fn table_exists(shamir: &ShamirDb, db: &str, repo: &str, table: &str) -> bool {
    shamir
        .get_db(db)
        .is_some_and(|instance| instance.has_table(repo, table))
}

pub(super) async fn index_exists(
    executor: &ShamirAdminExecutor,
    repo: &str,
    table: &str,
    index: &str,
) -> bool {
    let Some(instance) = executor.shamir.get_db(&executor.db_name) else {
        return false;
    };
    let Ok(table_manager) = instance.get_table(repo, table).await else {
        return false;
    };
    table_manager.any_index_exists(index).await
}

/// The stored declarative schema rule set of a table, de-interned to DTOs.
pub(super) async fn stored_rules(
    executor: &ShamirAdminExecutor,
    repo: &str,
    table: &str,
) -> Result<Vec<FieldRuleDto>, BatchError> {
    let record = executor
        .shamir
        .system_store()
        .load_table_record(&executor.db_name, repo, table)
        .await
        .map_err(map_store_error)?
        .ok_or_else(|| {
            coded(
                "schema_migration_postcondition_failed",
                format!(
                    "table '{}/{repo}/{table}' has no catalogue record",
                    executor.db_name
                ),
            )
        })?;
    let interner_manager = executor
        .shamir
        .resolve_repo_interner(&executor.db_name, repo)
        .await
        .map_err(map_store_error)?;
    let interner = interner_manager
        .get()
        .await
        .map_err(|error| plain(error.to_string()))?;
    checked_catalogue_rules(record.get(SCHEMA_FIELD), interner)
}

/// Exact §4.2 postcondition of one operation. `created_from_absent` carries
/// the create-time pre-image (a create whose object already existed is a
/// conflict, never `if_not_exists` success). Returns the completion evidence
/// string on success.
pub(super) async fn verify_postcondition(
    executor: &ShamirAdminExecutor,
    db: &str,
    operation: &SchemaMigrationOp,
    created_from_absent: bool,
) -> Result<String, BatchError> {
    let failed = |detail: String| coded("schema_migration_postcondition_failed", detail);
    match operation {
        SchemaMigrationOp::CreateTable(op) => {
            if !created_from_absent {
                return Err(failed(format!(
                    "create table '{}/{}': object already existed before the operation \
                     (conflicting definition, never an if_not_exists success)",
                    op.repo, op.create_table
                )));
            }
            if !table_exists(&executor.shamir, db, &op.repo, &op.create_table) {
                return Err(failed(format!(
                    "create table '{}/{}': object is absent after execution",
                    op.repo, op.create_table
                )));
            }
            Ok(format!("created table '{}/{}'", op.repo, op.create_table))
        }
        SchemaMigrationOp::DropTable(op) => {
            if table_exists(&executor.shamir, db, &op.repo, &op.drop_table) {
                return Err(failed(format!(
                    "drop table '{}/{}': object is still present after execution",
                    op.repo, op.drop_table
                )));
            }
            Ok(format!("dropped table '{}/{}'", op.repo, op.drop_table))
        }
        SchemaMigrationOp::RenameTable(op) => {
            if table_exists(&executor.shamir, db, &op.repo, &op.rename_table)
                || !table_exists(&executor.shamir, db, &op.repo, &op.to)
            {
                return Err(failed(format!(
                    "rename table '{}/{}' → '{}': source present or destination absent",
                    op.repo, op.rename_table, op.to
                )));
            }
            Ok(format!(
                "renamed table '{}/{}' → '{}'",
                op.repo, op.rename_table, op.to
            ))
        }
        SchemaMigrationOp::CreateIndex(op) => {
            if !created_from_absent {
                return Err(failed(format!(
                    "create index '{}' on '{}/{}': index already existed before the \
                     operation (conflicting definition)",
                    op.create_index, op.repo, op.table
                )));
            }
            if !Box::pin(index_exists(
                executor,
                &op.repo,
                &op.table,
                &op.create_index,
            ))
            .await
            {
                return Err(failed(format!(
                    "create index '{}' on '{}/{}': index is absent after execution",
                    op.create_index, op.repo, op.table
                )));
            }
            Ok(format!(
                "created index '{}' on '{}/{}'",
                op.create_index, op.repo, op.table
            ))
        }
        SchemaMigrationOp::DropIndex(op) => {
            if Box::pin(index_exists(executor, &op.repo, &op.table, &op.drop_index)).await {
                return Err(failed(format!(
                    "drop index '{}' on '{}/{}': index is still present after execution",
                    op.drop_index, op.repo, op.table
                )));
            }
            Ok(format!(
                "dropped index '{}' on '{}/{}'",
                op.drop_index, op.repo, op.table
            ))
        }
        SchemaMigrationOp::RenameIndex(op) => {
            if Box::pin(index_exists(
                executor,
                &op.repo,
                &op.table,
                &op.rename_index,
            ))
            .await
                || !Box::pin(index_exists(executor, &op.repo, &op.table, &op.to)).await
            {
                return Err(failed(format!(
                    "rename index '{}' → '{}' on '{}/{}': source present or destination absent",
                    op.rename_index, op.to, op.repo, op.table
                )));
            }
            Ok(format!(
                "renamed index '{}' → '{}' on '{}/{}'",
                op.rename_index, op.to, op.repo, op.table
            ))
        }
        SchemaMigrationOp::SetTableSchema(op) => {
            let rules = stored_rules(executor, &op.repo, &op.set_table_schema).await?;
            if rules != op.schema {
                return Err(failed(format!(
                    "set table schema '{}/{}': stored rule set differs from the intended one",
                    op.repo, op.set_table_schema
                )));
            }
            Ok(format!(
                "set schema of table '{}/{}' ({} rules)",
                op.repo,
                op.set_table_schema,
                rules.len()
            ))
        }
        SchemaMigrationOp::AddSchemaRule(op) => {
            let rules = stored_rules(executor, &op.repo, &op.add_schema_rule).await?;
            if !rules.iter().any(|rule| rule == &op.rule) {
                return Err(failed(format!(
                    "add schema rule '{}/{}': intended rule is not stored verbatim",
                    op.repo, op.add_schema_rule
                )));
            }
            Ok(format!(
                "added schema rule to table '{}/{}'",
                op.repo, op.add_schema_rule
            ))
        }
        SchemaMigrationOp::RemoveSchemaRule(op) => {
            let rules = stored_rules(executor, &op.repo, &op.remove_schema_rule).await?;
            if rules.iter().any(|rule| rule.path == op.path) {
                return Err(failed(format!(
                    "remove schema rule '{}/{}': a rule with the removed path is still stored",
                    op.repo, op.remove_schema_rule
                )));
            }
            Ok(format!(
                "removed schema rule from table '{}/{}'",
                op.repo, op.remove_schema_rule
            ))
        }
        _ => Err(plain("not a table/index/schema-rule operation".to_string())),
    }
}
