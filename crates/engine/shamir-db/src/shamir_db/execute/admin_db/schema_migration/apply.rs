//! Application-schema migration APPLY arm (release-1 R1c step 2b-3a,
//! design §4.1–§4.3, §5.2, §5.3).
//!
//! This file owns the ADMISSION entry and its admission helpers; the fenced
//! operation runner and its per-operation DDL execution + exact
//! postconditions live in `apply_operations.rs`, and the journal append /
//! error mapping / result wire encoding live in `apply_journal.rs`.
//!
//! Executes the deterministic table/index/schema-rule subset of a submitted
//! migration body under the durable fence of step 2a, with per-operation
//! journal intent → one bounded DDL → exact postcondition → completion, and
//! a final end-state verification before the durable `applied` event and
//! fence release.
//!
//! Admission order (nothing executes before ALL of it holds):
//! 1. `Manage` on the database (dispatcher, normal `authorize_access` path);
//! 2. primary-only engine-side check (`node_read_only` flag lowered by the
//!    server; the server's own follower gate stays the first refusal);
//! 3. serving identity == request target;
//! 4. server-recomputed canonical checksum of the submitted body;
//! 5. already-applied ordinal: identical id/ordinal/checksum → the stored
//!    applied result as a no-op; changed body → refused without any write;
//! 6. in-flight owner / in-doubt journal (R1c step 2b-3b resume): an
//!    unresolved entry whose id/ordinal/server-recomputed checksum match the
//!    submission RESUMES (steps 7–9 unchanged, then the durable fence is
//!    acquired with `recovery = true` at the CURRENT durable generation and
//!    the runner reconciles per-operation before executing anything new);
//!    with the in-process gate already held → `schema_migration_in_progress`
//!    (a live runner of this db process is never taken over); any other
//!    mismatch → the fresh-path stable refusal codes, nothing written;
//! 7. expected predecessor == durable journal head; ordinal contiguous with
//!    the applied prefix (same refusals and stable codes as plan);
//! 8. function/validator-family operations → one typed, documented refusal
//!    (step 3b executes them); HMAC-required destructive operations →
//!    `hmac_required` refusal BEFORE any intent (the engine never holds the
//!    session HMAC key, so it cannot verify the tag itself);
//! 9. repository existence of every operation's target repo and §5.2
//!    authorization of every underlying DDL target through the
//!    normal access path — before the fence and before any journal write;
//! 10. the durable fence CAS with `expected_fence_generation` (recovery
//!     takes over the OWNED fence at the journal's current generation)
//!     plus the in-process per-database gate (§4.3).
//!
//! Any admission refusal writes nothing. Journal write/sync/read-back
//! failures stop the run before the next effect and are reported in doubt —
//! never success from in-memory state. A failed postcondition records no
//! completion and returns `schema_migration_postcondition_failed` with the
//! journal left resumable (resume itself is step 3b).

use shamir_query_types::admin::types::schema_migration::{
    canonical_body_checksum, SchemaMigrationApplyRequest, SchemaMigrationOp,
    SchemaMigrationPredecessor,
};

use crate::access::{Action, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::shamir_db::system_store::{DatabaseIdentity, FormatEpoch, SchemaMigrationContext};

use super::super::admin_dispatch::ShamirAdminExecutor;
use super::apply_journal::{applied_value, map_fence_error, plain};
use super::apply_operations::{run_operations, FencedRun};
use super::plan::{
    applied_head, coded, map_store_error, operation_kind_and_target, submission_body,
};

/// Stable documented refusal message for function/validator operations:
/// step 3b executes them (design §4.4 preflight/dependency work lands there).
/// The fixed `schema_migration_operation_deferred: ` PREFIX is part of the
/// contract: the refusal carries no stable wire code, so callers and tests
/// identify it by this literal prefix; a wording change behind the prefix is
/// free, a prefix change is a breaking contract change.
pub(super) const FUNCTION_FAMILY_DEFERRED: &str = "schema_migration_operation_deferred: \
     function and validator operations are not executed by application-schema apply in \
     R1c step 2b-3a; they are refused before any effect and will be executed by step 2b-3b";

/// The apply entry point. Called from the dispatcher arm (already `Manage`-
/// authorized) through `Box::pin` so the many DDL-handler awaits live on the
/// heap, not in the dispatcher's own future (Windows debug-build overflow).
pub(in crate::shamir_db::execute) async fn apply_application_schema(
    executor: &ShamirAdminExecutor,
    request: &SchemaMigrationApplyRequest,
) -> Result<QueryResult, BatchError> {
    let db = executor.db_name.clone();

    // (2) Engine-side primary-only check. The server follower gate has
    // already refused; this is the fail-closed second layer (§4.5).
    if executor.shamir.node_read_only() {
        return Err(coded(
            "schema_migration_follower_read_only",
            "application-schema apply is refused on a follower/read replica \
             (no consistent journal+schema view)"
                .to_string(),
        ));
    }

    let store = executor.shamir.system_store();
    let serving = store
        .schema_migration_identity(&db)
        .await
        .map_err(map_store_error)?;

    // (3) Serving identity == request target.
    let target = DatabaseIdentity(request.target.database_identity);
    if target != serving {
        return Err(coded(
            "schema_migration_identity_mismatch",
            format!(
                "apply targets database identity {target} but the serving database is {serving}"
            ),
        ));
    }

    let history = store
        .read_schema_migration_history(serving)
        .await
        .map_err(map_store_error)?;
    let submission = &request.migration;
    let body = submission_body(submission);

    // (4) Server-recomputed canonical checksum; `validate` mirrors plan's
    // typed-admission re-check.
    body.validate().map_err(|error| plain(error.to_string()))?;
    let recomputed = canonical_body_checksum(&body).map_err(|error| plain(error.to_string()))?;
    if recomputed != submission.checksum {
        return Err(coded(
            "schema_migration_checksum_mismatch",
            format!(
                "submitted checksum does not match the canonical body of migration {}",
                submission.metadata.id
            ),
        ));
    }

    // (5) Already-applied ordinal: identical body → durable no-op result;
    // changed body → refusal without any write.
    if let Some(applied) = history
        .applied
        .iter()
        .find(|event| event.migration.ordinal == submission.metadata.ordinal)
    {
        if applied.migration == submission.metadata && applied.checksum == recomputed {
            return Ok(applied_value(
                request.protocol_version,
                submission.metadata.id.clone(),
                submission.metadata.ordinal,
                applied.checksum,
                applied.timestamp_unix_micros,
                applied.fence_generation,
            ));
        }
        return Err(coded(
            "schema_migration_checksum_mismatch",
            format!(
                "migration {} redefines applied ordinal {} with a different body",
                submission.metadata.id, submission.metadata.ordinal
            ),
        ));
    }

    // (6) Unresolved journal (resume/reconcile, R1c step 2b-3b). A live
    // runner of THIS db process (in-process gate held) is never taken over;
    // a matching unresolved migration (same id, same ordinal, checksum equal
    // to the server-recomputed canonical checksum) resumes; any other
    // mismatch is refused with the fresh-path stable codes, writing nothing.
    let mut resume = false;
    if let Some(event) = history.unresolved.last() {
        if executor
            .shamir
            .schema_migration_gate_generation(&db)
            .is_some()
        {
            return Err(coded(
                "schema_migration_in_progress",
                format!(
                    "migration {} (ordinal {}) is unresolved and a runner already holds \
                     the in-process gate for database '{db}'",
                    event.migration.id, event.migration.ordinal
                ),
            ));
        }
        if event.migration.id != submission.metadata.id
            || event.migration.ordinal != submission.metadata.ordinal
        {
            return Err(coded(
                "schema_migration_predecessor_mismatch",
                format!(
                    "migration {} (ordinal {}) is unresolved; apply cannot substitute \
                     migration {} (ordinal {})",
                    event.migration.id,
                    event.migration.ordinal,
                    submission.metadata.id,
                    submission.metadata.ordinal
                ),
            ));
        }
        if event.checksum != recomputed {
            return Err(coded(
                "schema_migration_checksum_mismatch",
                format!(
                    "migration {} is unresolved with a different body checksum",
                    submission.metadata.id
                ),
            ));
        }
        resume = true;
    }

    // (7) Expected predecessor == durable journal head; ordinal contiguous.
    let head = applied_head(&history);
    let expected = SchemaMigrationPredecessor {
        migration_id: head.map(|(id, _, _)| id.to_string()),
        ordinal: head.map_or(0, |(_, ordinal, _)| ordinal),
        checksum: head.map_or([0; 32], |(_, _, checksum)| checksum),
    };
    if request.expected_predecessor != expected {
        return Err(coded(
            "schema_migration_predecessor_mismatch",
            format!(
                "expected predecessor {}/{}/{:?} does not match the journal head",
                expected.migration_id.as_deref().unwrap_or("-"),
                expected.ordinal,
                expected.checksum
            ),
        ));
    }
    let expected_ordinal = expected.ordinal.checked_add(1).ok_or_else(|| {
        coded(
            "schema_migration_gap",
            "applied ordinal prefix is exhausted".to_string(),
        )
    })?;
    if submission.metadata.ordinal != expected_ordinal {
        return Err(coded(
            "schema_migration_gap",
            format!(
                "migration {} has ordinal {} but the journal continues at {expected_ordinal}",
                submission.metadata.id, submission.metadata.ordinal
            ),
        ));
    }

    // (8) Function/validator operations are step 3b; HMAC-required destructive
    // operations cannot be verified engine-side and are refused before intent.
    for operation in &body.operations {
        if is_function_or_validator(operation) {
            return Err(BatchError::QueryError {
                alias: String::new(),
                message: FUNCTION_FAMILY_DEFERRED.to_string(),
                code: None,
            });
        }
        if let Some(op) = fence_batch_op(operation) {
            if shamir_query_types::hmac::hmac_policy(&op)
                == shamir_query_types::hmac::HmacPolicy::Required
            {
                return Err(coded(
                    "hmac_required",
                    format!(
                        "operation '{}' of migration {} is HMAC-gated; migration bodies \
                         cannot carry verifiable session-HMAC tags in this step, so the \
                         migration is stopped before any intent is written",
                        operation_kind_and_target(operation).0,
                        submission.metadata.id
                    ),
                ));
            }
        }
    }

    // (9) A body naming a repository the serving database instance does not
    // have is knowable at admission: refuse it here (the handlers' own
    // `NotFound` → wire code `not_found`), BEFORE the fence and before the
    // durable `started` event would strand the journal in doubt. §5.2:
    // authorization of every underlying DDL target through the normal access
    // path — the migration-level `Manage` never replaces the DDL checks.
    let migration_id = submission.metadata.id.clone();
    for operation in &body.operations {
        preflight_repo_exists(executor, &db, operation, &migration_id)?;
        preflight_authorized(executor, operation).await?;
    }

    // (10) In-process gate fast-fail, then the durable fence CAS. The
    // acquire also re-validates id/ordinal/checksum against the journal and
    // persists+syncs the `started` event WITH the complete checksum-verified
    // body before any effect (§4.1 runner sequence).
    if executor
        .shamir
        .schema_migration_gate_generation(&db)
        .is_some()
    {
        return Err(coded(
            "schema_migration_in_progress",
            format!(
                "an application-schema migration already holds the maintenance fence \
                 for database '{db}'"
            ),
        ));
    }
    let context = SchemaMigrationContext::new(executor.actor.clone(), FormatEpoch::Unknown);
    // Recovery CASes the fence at the CURRENT durable generation, i.e. the
    // generation of the latest journal event (the control-record validation
    // pins control.generation to exactly that value).
    let expected_generation = if resume {
        history
            .events
            .last()
            .map_or(0, |event| event.fence_generation)
    } else {
        request.expected_fence_generation
    };
    let generation = store
        .acquire_schema_migration_fence(
            serving,
            expected_generation,
            submission.metadata.clone(),
            body.clone(),
            resume,
            &context,
        )
        .await
        .map_err(map_fence_error)?;

    // Fence owned durably: the runner holds the in-process gate under an
    // RAII guard so EVERY exit path releases it.
    run_operations(FencedRun {
        executor,
        serving,
        db: &db,
        submission,
        body: &body,
        protocol_version: request.protocol_version,
        generation,
        resume,
        context: &context,
    })
    .await
}

/// Convert an admitted migration operation to its `BatchOp` shape for the
/// HMAC-policy preflight. `None` for the function/validator families (they
/// are refused before this runs) — kept total for exhaustiveness.
fn fence_batch_op(operation: &SchemaMigrationOp) -> Option<BatchOp> {
    match operation {
        SchemaMigrationOp::CreateTable(op) => Some(BatchOp::CreateTable((**op).clone())),
        SchemaMigrationOp::DropTable(op) => Some(BatchOp::DropTable((**op).clone())),
        SchemaMigrationOp::RenameTable(op) => Some(BatchOp::RenameTable((**op).clone())),
        SchemaMigrationOp::CreateIndex(op) => Some(BatchOp::CreateIndex((**op).clone())),
        SchemaMigrationOp::DropIndex(op) => Some(BatchOp::DropIndex((**op).clone())),
        SchemaMigrationOp::RenameIndex(op) => Some(BatchOp::RenameIndex((**op).clone())),
        SchemaMigrationOp::SetTableSchema(op) => Some(BatchOp::SetTableSchema((**op).clone())),
        SchemaMigrationOp::AddSchemaRule(op) => Some(BatchOp::AddSchemaRule((**op).clone())),
        SchemaMigrationOp::RemoveSchemaRule(op) => Some(BatchOp::RemoveSchemaRule((**op).clone())),
        _ => None,
    }
}

fn is_function_or_validator(operation: &SchemaMigrationOp) -> bool {
    matches!(
        operation,
        SchemaMigrationOp::CreateFunction(_)
            | SchemaMigrationOp::DropFunction(_)
            | SchemaMigrationOp::RenameFunction(_)
            | SchemaMigrationOp::CheckFunction(_)
            | SchemaMigrationOp::AlterFunction(_)
            | SchemaMigrationOp::CreateValidator(_)
            | SchemaMigrationOp::DropValidator(_)
            | SchemaMigrationOp::RenameValidator(_)
            | SchemaMigrationOp::BindValidator(_)
            | SchemaMigrationOp::UnbindValidator(_)
    )
}

/// Repo name each admitted operation addresses (`None` for the
/// function/validator family, which is refused earlier). Every admitted
/// variant carries a `repo` field naming its repository (RenameTable's
/// source and destination share it).
fn operation_repo(operation: &SchemaMigrationOp) -> Option<&str> {
    match operation {
        SchemaMigrationOp::CreateTable(op) => Some(&op.repo),
        SchemaMigrationOp::DropTable(op) => Some(&op.repo),
        SchemaMigrationOp::RenameTable(op) => Some(&op.repo),
        SchemaMigrationOp::CreateIndex(op) => Some(&op.repo),
        SchemaMigrationOp::DropIndex(op) => Some(&op.repo),
        SchemaMigrationOp::RenameIndex(op) => Some(&op.repo),
        SchemaMigrationOp::SetTableSchema(op) => Some(&op.repo),
        SchemaMigrationOp::AddSchemaRule(op) => Some(&op.repo),
        SchemaMigrationOp::RemoveSchemaRule(op) => Some(&op.repo),
        _ => None, // function/validator family refused earlier
    }
}

/// Admission preflight: the repository each operation addresses must exist
/// on the serving database instance. Uses the same cheap existence check
/// the DDL handlers resolve through (`DbInstance::get_repo`), and the same
/// wire code `map_store_error` gives an engine `NotFound` (`not_found`).
fn preflight_repo_exists(
    executor: &ShamirAdminExecutor,
    db: &str,
    operation: &SchemaMigrationOp,
    migration_id: &str,
) -> Result<(), BatchError> {
    let Some(repo) = operation_repo(operation) else {
        return Ok(());
    };
    let exists = executor
        .shamir
        .get_db(db)
        .is_some_and(|instance| instance.get_repo(repo).is_some());
    if !exists {
        return Err(coded(
            "not_found",
            format!(
                "migration {migration_id} names repository '{repo}' which does not \
                 exist on the serving database '{db}'"
            ),
        ));
    }
    Ok(())
}

/// §5.2 authorization preflight mirroring each DDL handler's own check
/// (Create on the store for create-table; Delete on the table for drop-table;
/// Write on the table for every other admitted op). The handlers re-run the
/// same checks unchanged — this preflight only moves a denial BEFORE the
/// operation intent is written.
async fn preflight_authorized(
    executor: &ShamirAdminExecutor,
    operation: &SchemaMigrationOp,
) -> Result<(), BatchError> {
    let db = &executor.db_name;
    let (path, action) = match operation {
        SchemaMigrationOp::CreateTable(op) => (
            ResourcePath::store(db.clone(), op.repo.clone()),
            Action::Create,
        ),
        SchemaMigrationOp::DropTable(op) => (
            ResourcePath::table(db.clone(), op.repo.clone(), op.drop_table.clone()),
            Action::Delete,
        ),
        SchemaMigrationOp::RenameTable(op) => (
            ResourcePath::table(db.clone(), op.repo.clone(), op.rename_table.clone()),
            Action::Write,
        ),
        SchemaMigrationOp::CreateIndex(op) => (
            ResourcePath::table(db.clone(), op.repo.clone(), op.table.clone()),
            Action::Write,
        ),
        SchemaMigrationOp::DropIndex(op) => (
            ResourcePath::table(db.clone(), op.repo.clone(), op.table.clone()),
            Action::Write,
        ),
        SchemaMigrationOp::RenameIndex(op) => (
            ResourcePath::table(db.clone(), op.repo.clone(), op.table.clone()),
            Action::Write,
        ),
        SchemaMigrationOp::SetTableSchema(op) => (
            ResourcePath::table(db.clone(), op.repo.clone(), op.set_table_schema.clone()),
            Action::Write,
        ),
        SchemaMigrationOp::AddSchemaRule(op) => (
            ResourcePath::table(db.clone(), op.repo.clone(), op.add_schema_rule.clone()),
            Action::Write,
        ),
        SchemaMigrationOp::RemoveSchemaRule(op) => (
            ResourcePath::table(db.clone(), op.repo.clone(), op.remove_schema_rule.clone()),
            Action::Write,
        ),
        _ => return Ok(()), // function/validator family refused earlier
    };
    executor
        .shamir
        .authorize_access(&executor.actor, &path, action)
        .await
        .map_err(|error| coded("access_denied", error.to_string()))
}
