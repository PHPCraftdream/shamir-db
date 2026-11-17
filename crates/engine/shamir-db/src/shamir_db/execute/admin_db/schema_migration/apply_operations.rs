//! The fenced operation runner of the application-schema APPLY arm
//! (release-1 R1c step 2b-3a, design §4.1–§4.3): per-operation journal
//! intent → one bounded DDL → exact postcondition → completion, then
//! end-state verification, the durable `applied` event, and fence release.
//! Admission lives in `apply.rs`; the journal append / error mapping / wire
//! encoding live in `apply_journal.rs`.

use crate::query::admin::{
    AddSchemaRuleOp, CreateIndexOp, CreateTableOp, DropIndexOp, DropTableOp, FieldRuleDto,
    RemoveSchemaRuleOp, SetTableSchemaOp,
};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::shamir_db::shamir_db::schema_management::SCHEMA_FIELD;
use crate::shamir_db::system_store::{
    DatabaseIdentity, JournalEventKind, SchemaMigrationContext, SchemaMigrationStep,
};
use crate::shamir_db::ShamirDb;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationBody, SchemaMigrationOp, SchemaMigrationSubmission,
};

use super::super::admin_dispatch::ShamirAdminExecutor;
use super::super::schema_codec::checked_catalogue_rules;
use super::apply::FUNCTION_FAMILY_DEFERRED;
use super::apply_journal::{
    append_step, applied_value, durability_error, plain, record_in_doubt,
    record_postcondition_failure, JournalRef,
};
use super::plan::{coded, map_store_error};

/// Parameter bundle for one fenced run: the runner takes this single struct
/// instead of a long argument list (rust-guard: no new `#[allow]`).
pub(super) struct FencedRun<'a> {
    pub(super) executor: &'a ShamirAdminExecutor,
    pub(super) serving: DatabaseIdentity,
    pub(super) db: &'a str,
    pub(super) submission: &'a SchemaMigrationSubmission,
    pub(super) body: &'a SchemaMigrationBody,
    pub(super) protocol_version: u16,
    pub(super) generation: u64,
    pub(super) context: &'a SchemaMigrationContext,
}

/// RAII release of the IN-PROCESS gate on every exit path, so a failed
/// migration never leaves the fence held in-process. The durable fence
/// generation is released separately through the step-2a API only after the
/// `applied` event is durable.
struct GateGuard<'a> {
    shamir: &'a ShamirDb,
    db: String,
    generation: u64,
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        self.shamir
            .release_schema_migration_gate(&self.db, self.generation);
    }
}

/// The fenced run: per operation persist intent → execute one bounded DDL →
/// verify the exact postcondition → persist completion; then end-state
/// verification, durable `applied`, and fence release. On any mid-flight
/// failure the journal is left resumable (intent without completion, or an
/// in-doubt event) and the failure is reported with the stable §5.3 code —
/// never `applied` from in-memory state.
pub(super) async fn run_operations(run: FencedRun<'_>) -> Result<QueryResult, BatchError> {
    let FencedRun {
        executor,
        serving,
        db,
        submission,
        body,
        protocol_version,
        generation,
        context,
    } = run;
    let store = executor.shamir.system_store();
    let metadata = &submission.metadata;
    let checksum = submission.checksum;
    let journal = JournalRef {
        serving,
        metadata,
        checksum,
        generation,
        context,
    };
    // Fence owned durably: hold the in-process gate under an RAII guard so
    // EVERY exit path releases it (mirrors the admission entry, which
    // acquired the durable fence before calling this runner).
    executor.shamir.hold_schema_migration_gate(db, generation);
    let _gate = GateGuard {
        shamir: &executor.shamir,
        db: db.to_string(),
        generation,
    };

    // After the first durable intent, a later journal failure means an
    // effect may already have occurred → in doubt, never plain unavailability.
    let mut intents_durable = false;

    for (position, operation) in body.operations.iter().enumerate() {
        let index = u32::try_from(position).map_err(|_| {
            coded(
                "schema_migration_gap",
                "operation index overflow".to_string(),
            )
        })?;

        // Persist + sync the operation intent BEFORE the effect (§4.1).
        append_step(
            executor,
            &journal,
            SchemaMigrationStep {
                kind: JournalEventKind::OperationIntent,
                operation_index: Some(index),
                evidence: None,
            },
        )
        .await
        .map_err(|error| durability_error(error, intents_durable))?;
        intents_durable = true;

        // Create postconditions need the pre-image: a pre-existing object is
        // a conflict (§4.2), never an `if_not_exists` success.
        let created_from_absent = match operation {
            SchemaMigrationOp::CreateTable(op) => {
                !table_exists(&executor.shamir, db, &op.repo, &op.create_table)
            }
            SchemaMigrationOp::CreateIndex(op) => {
                !Box::pin(index_exists(
                    executor,
                    &op.repo,
                    &op.table,
                    &op.create_index,
                ))
                .await
            }
            _ => true,
        };

        // Execute exactly one bounded DDL operation through the EXISTING
        // handler paths — existing validation, per-target authorization and
        // HMAC gates apply unchanged. Box::pin keeps each handler's future
        // off this runner's own state machine.
        if let Err(error) = Box::pin(execute_operation(executor, operation)).await {
            // The effect's outcome is uncertain: record in doubt (best
            // effort) and stop before the next effect, journal resumable.
            return Err(record_in_doubt(
                executor,
                &journal,
                format!("operation {index} failed before its postcondition: {error}"),
            )
            .await);
        }

        // Verify the exact postcondition; a failure records no completion.
        let evidence = match Box::pin(verify_postcondition(
            executor,
            db,
            operation,
            created_from_absent,
        ))
        .await
        {
            Ok(evidence) => evidence,
            Err(error) => {
                return Err(record_postcondition_failure(
                    executor,
                    &journal,
                    format!("operation {index} postcondition failed: {error}"),
                )
                .await);
            }
        };

        // Persist + sync the operation completion only after the observed
        // postcondition (never record complete before it is observed).
        append_step(
            executor,
            &journal,
            SchemaMigrationStep {
                kind: JournalEventKind::OperationComplete,
                operation_index: Some(index),
                evidence: Some(evidence),
            },
        )
        .await
        .map_err(|error| durability_error(error, intents_durable))?;
    }

    // End-state invariants: every operation's postcondition still holds.
    for operation in &body.operations {
        Box::pin(verify_postcondition(executor, db, operation, true)).await?;
    }

    // Durable `applied` — only after all completions and final checks.
    let applied_event = append_step(
        executor,
        &journal,
        SchemaMigrationStep {
            kind: JournalEventKind::Applied,
            operation_index: None,
            evidence: Some(format!(
                "operations={}; end-state verified",
                body.operations.len()
            )),
        },
    )
    .await
    .map_err(|error| durability_error(error, intents_durable))?;

    // Release the durable fence through the step-2a API (it appends the
    // FenceReleased event and CASes the control record to `released`).
    store
        .release_schema_migration_fence(serving, generation, metadata.clone(), checksum, context)
        .await
        .map_err(|error| durability_error(error, intents_durable))?;

    Ok(applied_value(
        protocol_version,
        metadata.id.clone(),
        metadata.ordinal,
        checksum,
        applied_event.timestamp_unix_micros,
        generation,
    ))
}

/// Execute exactly one bounded DDL operation by delegating to the existing
/// admin DDL handler — no DDL is reimplemented here.
async fn execute_operation(
    executor: &ShamirAdminExecutor,
    operation: &SchemaMigrationOp,
) -> Result<(), BatchError> {
    match operation {
        SchemaMigrationOp::CreateTable(op) => {
            let op: &CreateTableOp = op;
            executor.handle_create_table(op).await?;
        }
        SchemaMigrationOp::DropTable(op) => {
            let op: &DropTableOp = op;
            executor.handle_drop_table(op).await?;
        }
        SchemaMigrationOp::RenameTable(op) => {
            executor
                .handle_rename_table(&BatchOp::RenameTable((**op).clone()))
                .await?;
        }
        SchemaMigrationOp::CreateIndex(op) => {
            let op: &CreateIndexOp = op;
            executor.handle_create_index(op).await?;
        }
        SchemaMigrationOp::DropIndex(op) => {
            let op: &DropIndexOp = op;
            executor.handle_drop_index(op).await?;
        }
        SchemaMigrationOp::RenameIndex(op) => {
            executor
                .handle_rename_index(&BatchOp::RenameIndex((**op).clone()))
                .await?;
        }
        SchemaMigrationOp::SetTableSchema(op) => {
            let op: &SetTableSchemaOp = op;
            executor.handle_set_table_schema(op).await?;
        }
        SchemaMigrationOp::AddSchemaRule(op) => {
            let op: &AddSchemaRuleOp = op;
            executor.handle_add_schema_rule(op).await?;
        }
        SchemaMigrationOp::RemoveSchemaRule(op) => {
            let op: &RemoveSchemaRuleOp = op;
            executor.handle_remove_schema_rule(op).await?;
        }
        // The function/validator families never reach execution: admission
        // refuses them with FUNCTION_FAMILY_DEFERRED before the fence.
        _ => return Err(plain(FUNCTION_FAMILY_DEFERRED.to_string())),
    }
    Ok(())
}

// ── postconditions (§4.2) ───────────────────────────────────────────────────

fn table_exists(shamir: &ShamirDb, db: &str, repo: &str, table: &str) -> bool {
    shamir
        .get_db(db)
        .is_some_and(|instance| instance.has_table(repo, table))
}

async fn index_exists(
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
async fn stored_rules(
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
async fn verify_postcondition(
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
        // Refused at admission; unreachable but kept total.
        _ => Err(plain(FUNCTION_FAMILY_DEFERRED.to_string())),
    }
}
