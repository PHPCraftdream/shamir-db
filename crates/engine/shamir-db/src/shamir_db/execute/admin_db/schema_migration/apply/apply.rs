//! Application-schema migration APPLY arm (release-1 R1c step 2b-3a,
//! design §4.1–§4.3, §5.2, §5.3).
//!
//! This file owns the ADMISSION entry and its admission helpers; the fenced
//! operation runner and its per-operation DDL execution + exact
//! postconditions live in `apply_operations.rs`, and the journal append /
//! error mapping / result wire encoding live in `apply_journal.rs`.
//!
//! Executes admitted table/index/schema-rule and function-family operations of a submitted
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
//! 8. Reject cross-database bindings and HMAC-required operations before
//!    any intent; defer binds because this fence cannot drain staged writes.
//! 9. repository existence of every operation's target repo and §5.2
//!    authorization of every underlying DDL target through the
//!    normal access path, plus read-only function checks before either fence;
//!    authorization that depends on ordered effects is done by the runner;
//!    acquire the server-wide function-family gate before resolving declared
//!    dependencies and before the durable CAS;
//! 10. the durable fence CAS with `expected_fence_generation` (recovery
//!     takes over the OWNED fence at the journal's current generation)
//!     plus the in-process per-database gate (§4.3). Under the same
//!     replication-control scope, AFTER the serving-slot reservation (its
//!     refusal keeps precedence), a body containing `RenameValidator`
//!     preflights the binding holders of every DISTINCT source name
//!     (brief 84 item 3): a foreign admission conflict is returned as the
//!     foreign target's code before any Started/Intent journal event is
//!     written (identity lookup has already synced), and the returned
//!     capability guard is retained for the whole run.
//!
//! Any admission refusal writes nothing. Journal write/sync/read-back
//! failures stop the run before the next effect and are reported in doubt —
//! never success from in-memory state. A failed postcondition records no
//! completion and returns `schema_migration_postcondition_failed` with the
//! journal left resumable (resume itself is step 3b).

use shamir_collections::TFxSet;
use shamir_query_types::admin::types::schema_migration::{
    canonical_body_checksum, SchemaMigrationApplyRequest, SchemaMigrationOp,
    SchemaMigrationPredecessor,
};

use std::collections::BTreeSet;

use crate::access::{Action, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::shamir_db::system_store::{
    DatabaseIdentity, FormatEpoch, JournalEventKind, SchemaMigrationContext,
};

use super::super::super::admin_dispatch::ShamirAdminExecutor;
use super::super::fence::SchemaMigrationGateGuard;
use super::super::function_family::admission::{
    authorize_family_operation, defer_bind, family_batch_op, is_function_family, preflight_checks,
    preflight_dependencies, validate_binding_scope,
};
use super::super::function_family::gate::FunctionFamilyGate;
use super::super::plan::{
    applied_head, coded, map_store_error, operation_kind_and_target, submission_body,
};
use super::apply_journal::{applied_value, map_fence_error, plain};
use super::apply_operations::{run_operations, FencedRun};

/// Test-only finding-1 rendezvous: a test scopes this task-local around the
/// retry task; apply parks the task after its identity read and before the
/// history read, until the gate is released (deterministic publication
/// ordering; the task-local keeps parallel tests independent).
#[cfg(test)]
pub(in crate::shamir_db) struct AppliedRetryPause {
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[cfg(test)]
impl AppliedRetryPause {
    pub(in crate::shamir_db) fn armed() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            reached: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        })
    }

    pub(in crate::shamir_db) async fn reached(&self) {
        self.reached.notified().await;
    }

    pub(in crate::shamir_db) fn release(&self) {
        self.release.notify_one();
    }

    /// The parked task signals the test it is parked, then waits for the
    /// test's release.
    pub(in crate::shamir_db) async fn park(&self) {
        self.reached.notify_one();
        self.release.notified().await;
    }
}

#[cfg(test)]
tokio::task_local! {
    pub(in crate::shamir_db) static APPLIED_RETRY_PAUSE: std::sync::Arc<AppliedRetryPause>;
}

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

    #[cfg(test)]
    {
        // Finding-1 rendezvous: a test scopes APPLIED_RETRY_PAUSE around the
        // retry task and parks it here, after the identity read and before
        // the history read, so ordering is deterministic.
        if let Ok(pause) = APPLIED_RETRY_PAUSE.try_with(std::sync::Arc::clone) {
            pause.park().await;
        }
    }

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

    // (5) Already-applied ordinal: identical body → stored result (after
    // clearing a matching Applied-tail fence if necessary); changed body → refusal.
    if let Some(applied) = history
        .applied
        .iter()
        .find(|event| event.migration.ordinal == submission.metadata.ordinal)
    {
        if applied.migration == submission.metadata && applied.checksum == recomputed {
            // The observed tail is either the `Applied` event (fence owned,
            // release pending) or the `FenceReleased` event that released it.
            // The retry must not acknowledge before the event it observed is
            // DURABLE: the journal publishes the committed transaction before
            // its explicit durability tail, so a visible event read while the
            // original runner's gate still pins this generation is not proof.
            // (i) One atomic slot snapshot refuses while that generation is
            // pinned; (ii) a retained matching in-doubt marker with an
            // Applied tail routes the retry through the real release; (iii)
            // every other acknowledgement performs the journal partition's
            // explicit durable tail itself — never a duplicate release just
            // to harvest a CAS conflict, and never a marker clear on
            // visibility alone.
            let observed_tail = history
                .events
                .last()
                .filter(|event| {
                    event.migration == applied.migration
                        && event.checksum == applied.checksum
                        && ((event.kind == JournalEventKind::Applied
                            && event.fence_generation == applied.fence_generation)
                            || event.kind == JournalEventKind::FenceReleased)
                })
                .map(|event| event.kind);
            if let Some(tail_kind) = observed_tail {
                // One replication-control scope classifies the whole slot:
                // identity revalidation, the pinning snapshot, and (Applied
                // tails only) the atomic release reservation. The durable
                // release and the tail run outside the lock.
                let (pinned, gate) = Box::pin(classify_applied_tail(
                    executor,
                    &db,
                    serving,
                    applied.fence_generation,
                    tail_kind == JournalEventKind::Applied,
                ))
                .await?;
                if pinned {
                    // The gate leaves `Live(generation)` only after the
                    // run's last durable flush returned, so the observed
                    // event is published but not proven durable.
                    return Err(coded(
                        "schema_migration_in_progress",
                        format!(
                            "migration {} (ordinal {}) is applied but its journal event \
                             is not yet proven durable: the live runner still holds the \
                             maintenance fence for database '{db}'",
                            submission.metadata.id, submission.metadata.ordinal
                        ),
                    ));
                }
                if let Some(mut gate) = gate {
                    gate.hold(applied.fence_generation);
                    let context =
                        SchemaMigrationContext::new(executor.actor.clone(), FormatEpoch::Unknown);
                    Box::pin(store.release_schema_migration_fence(
                        serving,
                        applied.fence_generation,
                        applied.migration.clone(),
                        applied.checksum,
                        &context,
                    ))
                    .await
                    .map_err(|error| super::apply_journal::durability_error(error, true))?;
                    gate.mark_succeeded();
                } else {
                    // No release route ran: perform the journal partition's
                    // explicit durable tail before acknowledging. A retained
                    // in-doubt marker stays; reopen reconciles it.
                    Box::pin(store.sync_schema_migration_journal_tail())
                        .await
                        .map_err(|error| super::apply_journal::durability_error(error, true))?;
                }
            }
            // A visible FenceReleased is not proof its durability tail finished.
            // Do not clear a retained in-doubt marker on this no-op; reopen
            // reconciles it against the recovered journal.
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
        // The SAME migration-only classification STATUS and PLAN use
        // (brief 87, item 4): live runners and Lifecycle reservations yield
        // `in_progress`, while a retained in-doubt marker falls through to
        // the identity/checksum resume checks below.
        if executor
            .shamir
            .schema_migration_admission_state_code(serving, &history)
            == Some("schema_migration_in_progress")
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

    // (8) Reject cross-database bindings before any other operation refusal.
    for operation in &body.operations {
        validate_binding_scope(&db, operation)?;
    }
    // HMAC-required operations cannot be verified engine-side.
    for operation in &body.operations {
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
    let last_intent = if resume {
        history
            .events
            .iter()
            .rev()
            .find(|event| {
                event.migration == submission.metadata
                    && event.kind == JournalEventKind::OperationIntent
            })
            .and_then(|event| event.operation_index)
            .map(|index| index as usize)
    } else {
        None
    };
    let migration_id = &submission.metadata.id;
    let mut introduced: TFxSet<&str> = TFxSet::with_hasher(shamir_collections::THasher::default());
    for (index, operation) in body.operations.iter().enumerate() {
        if is_function_family(operation) {
            // The runner must authorize journaled effects only after proof,
            // or authorize their source before executing. For names introduced
            // earlier in the body, defer the source check until the actual ACL
            // exists; absent-resource defaults are not proof of authority.
            let source = match operation {
                SchemaMigrationOp::RenameFunction(op) => Some(op.rename_function.as_str()),
                SchemaMigrationOp::AlterFunction(op) => Some(op.alter_function.as_str()),
                _ => None,
            };
            let defer_source = source.is_some_and(|name| {
                last_intent.is_some_and(|last| index <= last) || introduced.contains(name)
            });
            if !defer_source {
                Box::pin(authorize_family_operation(executor, operation)).await?;
            } else if matches!(operation, SchemaMigrationOp::RenameFunction(_)) {
                Box::pin(executor.shamir.authorize_access(
                    &executor.actor,
                    &ResourcePath::FunctionNamespace,
                    Action::Create,
                ))
                .await
                .map_err(|error| coded("access_denied", error.to_string()))?;
            }
            defer_bind(operation)?;
        }
        preflight_repo_exists(executor, &db, operation, migration_id)?;
        Box::pin(preflight_authorized(executor, operation)).await?;
        match operation {
            SchemaMigrationOp::CreateFunction(op) => {
                introduced.insert(&op.create_function);
            }
            SchemaMigrationOp::RenameFunction(op) => {
                introduced.remove(op.rename_function.as_str());
                introduced.insert(&op.to);
            }
            _ => {}
        }
    }
    Box::pin(preflight_checks(executor, &body.operations)).await?;

    // Even without a function-family operation, another migration could rename
    // a declared endpoint. Hold the gate from lookup through the runner.
    let _function_gate = (!body.function_dependencies.is_empty()
        || body.operations.iter().any(is_function_family))
    .then(|| FunctionFamilyGate::acquire(&executor.shamir.function_family_migration_gate))
    .transpose()?;
    Box::pin(preflight_dependencies(
        executor,
        &body.function_dependencies,
        true,
    ))
    .await?;

    // Status/plan/fresh-apply classification is observational before atomic
    // reservation. Resume and applied-tail retry preserve their special paths.
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
    let (mut gate, rename_migration) = {
        let _control = executor.shamir.lock_replication_control().await;
        validate_serving_identity(executor, &db, serving)?;
        // Fresh APPLY shares STATUS/PLAN classification; the atomic
        // reservation remains decisive if an owner races afterward.
        if !resume {
            if let Some(code) = executor
                .shamir
                .schema_migration_admission_state_code(serving, &history)
            {
                return Err(coded(
                    code,
                    format!("database '{db}' has a migration state that prevents a fresh apply"),
                ));
            }
        }
        let gate = executor
            .shamir
            .reserve_schema_migration_run(serving, resume.then_some(expected_generation))
            .map_err(|code| {
                coded(
                    code,
                    format!(
                        "an application-schema migration already holds the maintenance fence \
                         for database '{db}'"
                    ),
                )
            })?;
        // Brief 84 item 3: a body containing RenameValidator reserves every
        // foreign binding holder of its distinct rename sources under this
        // replication-control scope, before the fence's first journal write;
        // the guard is released with the run (`FencedRun::rename`).
        let rename_migration = if body
            .operations
            .iter()
            .any(|operation| matches!(operation, SchemaMigrationOp::RenameValidator(_)))
        {
            let sources: Vec<String> = body
                .operations
                .iter()
                .filter_map(|operation| match operation {
                    SchemaMigrationOp::RenameValidator(op) => Some(op.rename_validator.clone()),
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            Some(
                Box::pin(executor.shamir.begin_validator_rename_migration(
                    &sources,
                    Some(serving),
                    &_control,
                ))
                .await
                .map_err(|error| error.into_batch_error(|e| plain(e.to_string())))?,
            )
        } else {
            None
        };
        (gate, rename_migration)
    };
    let context = SchemaMigrationContext::new(executor.actor.clone(), FormatEpoch::Unknown);
    let generation = Box::pin(store.acquire_schema_migration_fence_tracked(
        (serving, expected_generation),
        submission.metadata.clone(),
        body.clone(),
        resume,
        &context,
        &mut |generation| gate.observe_acquire_commit(generation),
    ))
    .await
    .map_err(map_fence_error)?;

    // Fence owned durably: the runner promotes the reservation before its first await.
    Box::pin(run_operations(
        FencedRun {
            executor,
            serving,
            db: &db,
            submission,
            body: &body,
            protocol_version: request.protocol_version,
            generation,
            resume,
            context: &context,
            rename: rename_migration,
        },
        gate,
    ))
    .await
}

/// One replication-control scope for the applied-tail no-op: identity
/// revalidation, the single atomic fence-slot snapshot, and (Applied tails
/// only, never after a visible FenceReleased) the release reservation.
/// The boolean pairs the pinning verdict with the optional reservation;
/// the caller performs the durable release or the explicit journal tail
/// OUTSIDE the lock, so the original runner never waits behind admission.
async fn classify_applied_tail<'a>(
    executor: &'a ShamirAdminExecutor,
    db: &str,
    serving: DatabaseIdentity,
    generation: u64,
    reserve: bool,
) -> Result<(bool, Option<SchemaMigrationGateGuard<'a>>), BatchError> {
    let _control = executor.shamir.lock_replication_control().await;
    validate_serving_identity(executor, db, serving)?;
    let pinned = executor
        .shamir
        .schema_migration_fence_pins(serving, generation);
    let gate = if reserve && !pinned {
        executor
            .shamir
            .reserve_applied_fence_release(serving, generation)
    } else {
        None
    };
    Ok((pinned, gate))
}

fn validate_serving_identity(
    executor: &ShamirAdminExecutor,
    db: &str,
    serving: DatabaseIdentity,
) -> Result<(), BatchError> {
    if executor.shamir.database_identity_for_fence(db) == Some(serving) {
        Ok(())
    } else {
        Err(coded(
            "schema_migration_identity_mismatch",
            format!("apply targets database identity {serving} but the serving database changed"),
        ))
    }
}

/// Convert an admitted migration operation to its `BatchOp` shape for the
/// HMAC-policy preflight, delegating function and validator shapes.
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
        _ => family_batch_op(operation),
    }
}

/// Repo targeted by table-family operations and table validator bindings.
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
        SchemaMigrationOp::BindValidator(op) => Some(&op.repo),
        SchemaMigrationOp::UnbindValidator(op) => Some(&op.repo),
        _ => None,
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
        _ => return Ok(()), // function-family authorization is handled separately
    };
    executor
        .shamir
        .authorize_access(&executor.actor, &path, action)
        .await
        .map_err(|error| coded("access_denied", error.to_string()))
}
