//! Application-schema migration PLAN arm (release-1 R1c step 2b-2).
//!
//! Validates a client-submitted ordered migration set against the durable
//! applied prefix (design §3.3, §4.1, §4.4) and returns the typed plan for
//! the next migration — checksums recomputed server-side, order proven
//! against the journal, effects and their blast radius listed — with NO side
//! effect: no journal event, no fence, no operation execution.
//!
//! Authorization is §5.2 `Manage` on the database, before any journal byte
//! is read. Refusals use the stable §5.3 codes only. Plan is primary-only;
//! the follower refusal sits in the server's single
//! `BatchOp::ApplicationSchema` gate (step 2b-1) and is not duplicated here.
//!
//! The apply arm remains a documented temporary error (step 2b-3).

use shamir_collections::TFxSet;
use shamir_query_types::admin::types::schema_migration::{
    canonical_body_checksum, SchemaMigrationBody, SchemaMigrationOp, SchemaMigrationPlanRequest,
    SchemaMigrationPlanResult, SchemaMigrationPlannedOperation, SchemaMigrationPredecessor,
    SchemaMigrationSubmission, SchemaMigrationWarning,
};
use shamir_types::types::common::new_map;

use crate::access::{Action, ResourcePath};
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::shamir_db::system_store::{DatabaseIdentity, JournalEventKind, MigrationHistory};
use crate::types::value::QueryValue;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::admin_result;

/// §5.2 authorization for the plan arm: `Manage` on the database through the
/// normal `authorize_access` path — no bypass — before any journal byte is
/// read. A caller with only `Read` is refused.
pub(super) async fn authorize_database_manage(
    executor: &ShamirAdminExecutor,
) -> Result<(), BatchError> {
    executor
        .shamir
        .authorize_access(
            &executor.actor,
            &ResourcePath::database(executor.db_name.clone()),
            Action::Manage,
        )
        .await
        .map_err(|error| BatchError::QueryError {
            alias: String::new(),
            message: error.to_string(),
            code: Some("access_denied".to_string()),
        })
}

/// Validate the submitted set against the durable journal and encode the
/// typed plan for the NEXT migration as the admin record. Writes nothing.
pub(super) async fn plan_application_schema(
    executor: &ShamirAdminExecutor,
    request: &SchemaMigrationPlanRequest,
) -> Result<QueryResult, BatchError> {
    let store = executor.shamir.system_store();
    let serving = store
        .schema_migration_identity(&executor.db_name)
        .await
        .map_err(map_store_error)?;
    let target = DatabaseIdentity(request.target.database_identity);
    if target != serving {
        return Err(coded(
            "schema_migration_identity_mismatch",
            format!(
                "plan targets database identity {target} but the serving database is {serving}"
            ),
        ));
    }
    let history = store
        .read_schema_migration_history(serving)
        .await
        .map_err(map_store_error)?;
    if let Some(event) = history.unresolved.last() {
        // §4.2: a journal with only `started` durably holds no effect yet —
        // an owned fence is reported in progress; once an operation intent
        // exists the migration is inside the uncertain window and is
        // reported in doubt.
        let code = match event.kind {
            JournalEventKind::Started => "schema_migration_in_progress",
            _ => "schema_migration_in_doubt",
        };
        return Err(coded(
            code,
            format!(
                "migration {} (ordinal {}) is unresolved; no plan is issued",
                event.migration.id, event.migration.ordinal
            ),
        ));
    }
    let head = applied_head(&history);
    let predecessor = SchemaMigrationPredecessor {
        migration_id: head.map(|(id, _, _)| id.to_string()),
        ordinal: head.map_or(0, |(_, ordinal, _)| ordinal),
        checksum: head.map_or([0; 32], |(_, _, checksum)| checksum),
    };
    if request.expected_predecessor != predecessor {
        return Err(coded(
            "schema_migration_predecessor_mismatch",
            format!(
                "expected predecessor {}/{}/{:?} does not match the journal head",
                predecessor.migration_id.as_deref().unwrap_or("-"),
                predecessor.ordinal,
                predecessor.checksum
            ),
        ));
    }
    let next = validate_submissions(&history, request, predecessor.ordinal)?;
    let result = plan_result(request.protocol_version, &predecessor, next);
    Ok(admin_result(plan_value(&result)))
}

/// Server-side validation of every submitted migration: nothing is trusted
/// from the client. Per body — typed admission (`validate`), server-recomputed
/// canonical checksum, and the exact continuation of the applied prefix with
/// no gap, duplicate, or reorder. A submission for an already-applied ordinal
/// must carry the identical id/ordinal/checksum; a changed body is refused
/// without any write. Returns the FIRST unapplied submission to plan.
fn validate_submissions<'a>(
    history: &MigrationHistory,
    request: &'a SchemaMigrationPlanRequest,
    last_applied_ordinal: u64,
) -> Result<&'a SchemaMigrationSubmission, BatchError> {
    let mut expected_ordinal = last_applied_ordinal.checked_add(1).ok_or_else(|| {
        coded(
            "schema_migration_gap",
            "applied ordinal prefix is exhausted".to_string(),
        )
    })?;
    let mut seen_ids: TFxSet<&str> = TFxSet::with_hasher(shamir_collections::THasher::default());
    let mut planned: Option<&'a SchemaMigrationSubmission> = None;
    for submission in &request.migrations {
        let body = submission_body(submission);
        body.validate().map_err(|error| BatchError::QueryError {
            alias: String::new(),
            message: error.to_string(),
            code: None,
        })?;
        let recomputed =
            canonical_body_checksum(&body).map_err(|error| BatchError::QueryError {
                alias: String::new(),
                message: error.to_string(),
                code: None,
            })?;
        if recomputed != submission.checksum {
            return Err(coded(
                "schema_migration_checksum_mismatch",
                format!(
                    "submitted checksum does not match the canonical body of migration {}",
                    submission.metadata.id
                ),
            ));
        }
        if !seen_ids.insert(submission.metadata.id.as_str()) {
            return Err(coded(
                "schema_migration_gap",
                format!("duplicate migration id {}", submission.metadata.id),
            ));
        }
        if let Some(applied) = history
            .applied
            .iter()
            .find(|event| event.migration.ordinal == submission.metadata.ordinal)
        {
            if applied.migration.id != submission.metadata.id || applied.checksum != recomputed {
                return Err(coded(
                    "schema_migration_checksum_mismatch",
                    format!(
                        "migration {} redefines applied ordinal {} with a different body",
                        submission.metadata.id, submission.metadata.ordinal
                    ),
                ));
            }
            continue; // already applied verbatim — not part of the plan
        }
        if submission.metadata.ordinal != expected_ordinal {
            return Err(coded(
                "schema_migration_gap",
                format!(
                    "migration {} has ordinal {} but the journal continues at {expected_ordinal}",
                    submission.metadata.id, submission.metadata.ordinal
                ),
            ));
        }
        expected_ordinal += 1;
        if planned.is_none() {
            planned = Some(submission);
        }
    }
    planned.ok_or_else(|| {
        coded(
            "schema_migration_gap",
            "the submitted set contains no unapplied migration to plan".to_string(),
        )
    })
}

/// The applied-prefix head: last applied migration (id, ordinal, checksum).
fn applied_head(history: &MigrationHistory) -> Option<(&str, u64, [u8; 32])> {
    history.applied.last().map(|event| {
        (
            event.migration.id.as_str(),
            event.migration.ordinal,
            event.checksum,
        )
    })
}

/// Rebuild the typed body view of a submission for checksum recomputation.
fn submission_body(submission: &SchemaMigrationSubmission) -> SchemaMigrationBody {
    SchemaMigrationBody {
        metadata: submission.metadata.clone(),
        function_dependencies: submission.function_dependencies.clone(),
        operations: submission.operations.clone(),
    }
}

/// Assemble the typed plan for the next migration: per-operation effect
/// entries with stable kind and target, warnings set by rule (§4.3, §4.4),
/// and the server-wide fence flag for server-global registrations.
fn plan_result(
    protocol_version: u16,
    predecessor: &SchemaMigrationPredecessor,
    submission: &SchemaMigrationSubmission,
) -> SchemaMigrationPlanResult {
    let mut operations = Vec::with_capacity(submission.operations.len());
    let mut has_function_op = false;
    let mut has_validator_op = false;
    for (index, operation) in submission.operations.iter().enumerate() {
        let (kind, target) = operation_kind_and_target(operation);
        let mut function_registration: Option<String> = None;
        let mut validator_registration: Option<String> = None;
        match operation {
            SchemaMigrationOp::CreateFunction(op) => {
                has_function_op = true;
                function_registration = Some(op.create_function.clone());
            }
            SchemaMigrationOp::DropFunction(op) => {
                has_function_op = true;
                function_registration = Some(op.drop_function.clone());
            }
            SchemaMigrationOp::RenameFunction(op) => {
                has_function_op = true;
                function_registration = Some(op.rename_function.clone());
            }
            SchemaMigrationOp::AlterFunction(op) => {
                has_function_op = true;
                function_registration = Some(op.alter_function.clone());
            }
            // Preflight-only: it registers nothing and has no target name.
            SchemaMigrationOp::CheckFunction(_) => {}
            SchemaMigrationOp::CreateValidator(op) => {
                has_validator_op = true;
                validator_registration = Some(op.create_validator.clone());
            }
            SchemaMigrationOp::DropValidator(op) => {
                has_validator_op = true;
                validator_registration = Some(op.drop_validator.clone());
            }
            SchemaMigrationOp::RenameValidator(op) => {
                has_validator_op = true;
                validator_registration = Some(op.rename_validator.clone());
            }
            SchemaMigrationOp::BindValidator(op) => {
                has_validator_op = true;
                validator_registration = Some(op.bind_validator.clone());
            }
            SchemaMigrationOp::UnbindValidator(op) => {
                has_validator_op = true;
                validator_registration = Some(op.unbind_validator.clone());
            }
            _ => {}
        }
        let dependent_functions = if function_registration.is_some() {
            submission
                .function_dependencies
                .iter()
                .map(|dependency| dependency.function.clone())
                .collect()
        } else {
            Vec::new()
        };
        let affected_database_registrations: Vec<String> = function_registration
            .into_iter()
            .chain(validator_registration)
            .collect();
        operations.push(SchemaMigrationPlannedOperation {
            operation_index: u32::try_from(index).unwrap_or(u32::MAX),
            operation_kind: kind.to_string(),
            target: target.to_string(),
            work_bound: None,
            dependent_functions,
            affected_database_registrations,
        });
    }
    let server_global = has_function_op || has_validator_op;
    let mut warnings = vec![SchemaMigrationWarning::MaintenanceFenceRequired];
    if server_global {
        warnings.push(SchemaMigrationWarning::ServerGlobalRegistrationBlastRadius);
        warnings.push(SchemaMigrationWarning::DependencyListMustBeReviewed);
    }
    SchemaMigrationPlanResult {
        protocol_version,
        migration: submission.metadata.clone(),
        checksum: submission.checksum,
        predecessor: predecessor.clone(),
        operations,
        total_work_bound: None,
        warnings,
        recovery_summary: Some(
            "no down migrations; recovery is resume under a new fence, operator \
             reconciliation, or restore of the pre-migration backup"
                .to_string(),
        ),
        requires_server_wide_fence: Some(server_global),
    }
}

/// Stable snake_case kind and target object of one admitted operation.
fn operation_kind_and_target(operation: &SchemaMigrationOp) -> (&'static str, &str) {
    match operation {
        SchemaMigrationOp::CreateTable(op) => ("create_table", &op.create_table),
        SchemaMigrationOp::DropTable(op) => ("drop_table", &op.drop_table),
        SchemaMigrationOp::RenameTable(op) => ("rename_table", &op.rename_table),
        SchemaMigrationOp::CreateIndex(op) => ("create_index", &op.create_index),
        SchemaMigrationOp::DropIndex(op) => ("drop_index", &op.drop_index),
        SchemaMigrationOp::RenameIndex(op) => ("rename_index", &op.rename_index),
        SchemaMigrationOp::SetTableSchema(op) => ("set_table_schema", &op.set_table_schema),
        SchemaMigrationOp::AddSchemaRule(op) => ("add_schema_rule", &op.add_schema_rule),
        SchemaMigrationOp::RemoveSchemaRule(op) => ("remove_schema_rule", &op.remove_schema_rule),
        SchemaMigrationOp::CreateFunction(op) => ("create_function", &op.create_function),
        SchemaMigrationOp::DropFunction(op) => ("drop_function", &op.drop_function),
        SchemaMigrationOp::RenameFunction(op) => ("rename_function", &op.rename_function),
        SchemaMigrationOp::CheckFunction(_) => ("check_function", ""),
        SchemaMigrationOp::AlterFunction(op) => ("alter_function", &op.alter_function),
        SchemaMigrationOp::CreateValidator(op) => ("create_validator", &op.create_validator),
        SchemaMigrationOp::DropValidator(op) => ("drop_validator", &op.drop_validator),
        SchemaMigrationOp::RenameValidator(op) => ("rename_validator", &op.rename_validator),
        SchemaMigrationOp::BindValidator(op) => ("bind_validator", &op.bind_validator),
        SchemaMigrationOp::UnbindValidator(op) => ("unbind_validator", &op.unbind_validator),
    }
}

/// Encode the typed result with the serde field names of
/// `SchemaMigrationPlanResult` — the same wire shape convention as the
/// status arm.
fn plan_value(result: &SchemaMigrationPlanResult) -> QueryValue {
    let mut metadata = new_map();
    metadata.insert(
        "id".to_string(),
        QueryValue::Str(result.migration.id.clone()),
    );
    metadata.insert(
        "ordinal".to_string(),
        QueryValue::Int(result.migration.ordinal as i64),
    );
    metadata.insert(
        "display_name".to_string(),
        QueryValue::Str(result.migration.display_name.clone()),
    );
    metadata.insert(
        "canonical_version".to_string(),
        QueryValue::Int(result.migration.canonical_version as i64),
    );
    let mut predecessor = new_map();
    if let Some(id) = &result.predecessor.migration_id {
        predecessor.insert("migration_id".to_string(), QueryValue::Str(id.clone()));
    }
    predecessor.insert(
        "ordinal".to_string(),
        QueryValue::Int(result.predecessor.ordinal as i64),
    );
    predecessor.insert(
        "checksum".to_string(),
        QueryValue::Bin(result.predecessor.checksum.to_vec()),
    );
    let operations = result
        .operations
        .iter()
        .map(|operation| {
            let mut map = new_map();
            map.insert(
                "operation_index".to_string(),
                QueryValue::Int(operation.operation_index as i64),
            );
            map.insert(
                "operation_kind".to_string(),
                QueryValue::Str(operation.operation_kind.clone()),
            );
            map.insert(
                "target".to_string(),
                QueryValue::Str(operation.target.clone()),
            );
            if let Some(work_bound) = operation.work_bound {
                map.insert("work_bound".to_string(), QueryValue::Int(work_bound as i64));
            }
            if !operation.dependent_functions.is_empty() {
                map.insert(
                    "dependent_functions".to_string(),
                    QueryValue::List(
                        operation
                            .dependent_functions
                            .iter()
                            .map(|name| QueryValue::Str(name.clone()))
                            .collect(),
                    ),
                );
            }
            if !operation.affected_database_registrations.is_empty() {
                map.insert(
                    "affected_database_registrations".to_string(),
                    QueryValue::List(
                        operation
                            .affected_database_registrations
                            .iter()
                            .map(|name| QueryValue::Str(name.clone()))
                            .collect(),
                    ),
                );
            }
            QueryValue::Map(map)
        })
        .collect();
    let mut value = new_map();
    value.insert(
        "protocol_version".to_string(),
        QueryValue::Int(result.protocol_version as i64),
    );
    value.insert("migration".to_string(), QueryValue::Map(metadata));
    value.insert(
        "checksum".to_string(),
        QueryValue::Bin(result.checksum.to_vec()),
    );
    value.insert("predecessor".to_string(), QueryValue::Map(predecessor));
    value.insert("operations".to_string(), QueryValue::List(operations));
    if let Some(total_work_bound) = result.total_work_bound {
        value.insert(
            "total_work_bound".to_string(),
            QueryValue::Int(total_work_bound as i64),
        );
    }
    value.insert(
        "warnings".to_string(),
        QueryValue::List(
            result
                .warnings
                .iter()
                .map(|warning| QueryValue::Str(warning_str(warning).to_string()))
                .collect(),
        ),
    );
    if let Some(recovery_summary) = &result.recovery_summary {
        value.insert(
            "recovery_summary".to_string(),
            QueryValue::Str(recovery_summary.clone()),
        );
    }
    if let Some(requires_server_wide_fence) = result.requires_server_wide_fence {
        value.insert(
            "requires_server_wide_fence".to_string(),
            QueryValue::Bool(requires_server_wide_fence),
        );
    }
    QueryValue::Map(value)
}

fn warning_str(warning: &SchemaMigrationWarning) -> &'static str {
    match warning {
        SchemaMigrationWarning::ServerGlobalRegistrationBlastRadius => {
            "server_global_registration_blast_radius"
        }
        SchemaMigrationWarning::MaintenanceFenceRequired => "maintenance_fence_required",
        SchemaMigrationWarning::RecoveryRequiresPostcondition => "recovery_requires_postcondition",
        SchemaMigrationWarning::DependencyListMustBeReviewed => "dependency_list_must_be_reviewed",
        SchemaMigrationWarning::DataDmlNotAdmitted => "data_dml_not_admitted",
    }
}

fn coded(code: &str, message: String) -> BatchError {
    BatchError::QueryError {
        alias: String::new(),
        message,
        code: Some(code.to_string()),
    }
}

/// §5.3 mapping — the same convention as the status arm's `map_store_error`:
/// journal corruption / durability failures are
/// `schema_migration_durability_unavailable`; a missing database keeps the
/// engine's existing `not_found`.
fn map_store_error(error: crate::DbError) -> BatchError {
    let code = match &error {
        crate::DbError::NotFound(_) => "not_found",
        _ => "schema_migration_durability_unavailable",
    };
    BatchError::QueryError {
        alias: String::new(),
        message: error.to_string(),
        code: Some(code.to_string()),
    }
}
