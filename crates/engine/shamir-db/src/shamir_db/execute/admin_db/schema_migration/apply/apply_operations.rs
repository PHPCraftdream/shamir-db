//! The fenced operation runner of the application-schema APPLY arm
//! (release-1 R1c step 2b-3a/2b-3b, design §4.1–§4.3): per-operation journal
//! intent → one bounded effect → exact postcondition → completion, then
//! end-state verification, the durable `applied` event, and fence release.
//! On a resume run, a reconcile pass first classifies each operation index
//! from the durable journal and finishes what the previous runner already
//! proved, without re-running completed work. Admission lives in `apply.rs`;
//! the journal append / error mapping / wire encoding live in
//! `apply_journal.rs`. A read-only CheckFunction runs before its intent.

use crate::query::admin::{
    AddSchemaRuleOp, CreateIndexOp, CreateTableOp, DropIndexOp, DropTableOp, RemoveSchemaRuleOp,
    SetTableSchemaOp,
};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::shamir_db::system_store::{
    DatabaseIdentity, JournalEventKind, SchemaMigrationContext, SchemaMigrationStep,
};
use crate::shamir_db::ShamirDb;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationBody, SchemaMigrationMetadata, SchemaMigrationOp, SchemaMigrationSubmission,
};

use super::super::super::admin_dispatch::ShamirAdminExecutor;
use super::super::function_family::{admission, execution, reconcile};
use super::super::plan::{coded, map_store_error};
use super::apply_journal::{
    append_step, applied_value, durability_error, plain, record_in_doubt,
    record_postcondition_failure, JournalRef,
};

#[path = "ddl_postconditions.rs"]
mod ddl_postconditions;

use self::ddl_postconditions::{index_exists, stored_rules, table_exists};

struct LiveGate<'a> {
    shamir: &'a ShamirDb,
    db: &'a str,
    generation: u64,
    succeeded: bool,
}

impl Drop for LiveGate<'_> {
    fn drop(&mut self) {
        if self.succeeded {
            self.shamir
                .release_schema_migration_gate(self.db, self.generation);
        } else {
            self.shamir
                .mark_schema_migration_gate_in_doubt(self.db, self.generation);
        }
    }
}

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
    /// Resume (recovery-fence takeover) run: reconcile from the journal
    /// before executing anything new (R1c step 2b-3b).
    pub(super) resume: bool,
    pub(super) context: &'a SchemaMigrationContext,
}

/// The fenced run: holds the IN-PROCESS gate for the whole run and converts
/// it to an in-doubt marker on error or cancellation. Only a successful
/// durable fence release clears the live gate.
pub(super) async fn run_operations(run: FencedRun<'_>) -> Result<QueryResult, BatchError> {
    let shamir = &run.executor.shamir;
    let db = run.db;
    let generation = run.generation;
    shamir.hold_schema_migration_gate(db, generation);
    let mut gate = LiveGate {
        shamir,
        db,
        generation,
        succeeded: false,
    };
    let result = Box::pin(run_operations_fenced(run)).await;
    gate.succeeded = result.is_ok();
    result
}

/// The fenced run body, called with the in-process gate already held: per
/// operation persist intent → execute one bounded DDL → verify the exact
/// postcondition → persist completion; then end-state verification, durable
/// `applied`, and fence release. On any mid-flight failure the journal is
/// left resumable (intent without completion, or an in-doubt event) and the
/// failure is reported with the stable §5.3 code — never `applied` from
/// in-memory state.
async fn run_operations_fenced(run: FencedRun<'_>) -> Result<QueryResult, BatchError> {
    let FencedRun {
        executor,
        serving,
        db,
        submission,
        body,
        protocol_version,
        generation,
        resume,
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
    // After the first durable intent, a later journal failure means an
    // effect may already have occurred → in doubt, never plain unavailability.
    let mut intents_durable = false;

    // Resume reconcile (§4.2): classify every operation index from the
    // durable journal rows BEFORE running anything new. The classify pass
    // is its own function, not part of the per-operation future.
    let mut states = if resume {
        Box::pin(classify_resume(
            executor,
            serving,
            metadata,
            checksum,
            body.operations.len(),
        ))
        .await?
    } else {
        vec![OperationProgress::default(); body.operations.len()]
    };

    for (position, (operation, progress)) in body.operations.iter().zip(&mut states).enumerate() {
        let index = u32::try_from(position).map_err(|_| {
            coded(
                "schema_migration_gap",
                "operation index overflow".to_string(),
            )
        })?;

        // Check/reconcile existing journal work before executing a new effect.
        // Box each step for Windows debug stacks.
        if !matches!(progress.state, JournalOpState::NotStarted) {
            intents_durable = true;
            progress.completion = Some(
                Box::pin(resume_operation(
                    executor, db, &journal, operation, index, progress,
                ))
                .await?,
            );
        } else {
            let (intent, completion) = Box::pin(run_operation(
                executor,
                db,
                &journal,
                operation,
                index,
                &mut intents_durable,
            ))
            .await?;
            progress.intent = intent;
            progress.completion = Some(completion);
        }
    }

    // Predicates are independent: a later drop, rename, or alter that
    // supersedes an earlier predicate can make this conservative check fail.
    for (operation, progress) in body.operations.iter().zip(&states) {
        Box::pin(verify_completed(executor, db, operation, progress)).await?;
    }

    // Durable `applied` — only after all completions and final checks.
    let applied_event = Box::pin(append_step(
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
    ))
    .await
    .map_err(|error| durability_error(error, intents_durable))?;

    // Release the durable fence through the step-2a API (it appends the
    // FenceReleased event and CASes the control record to `released`).
    Box::pin(store.release_schema_migration_fence(
        serving,
        generation,
        metadata.clone(),
        checksum,
        context,
    ))
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

/// Recheck the exact effect of a completed operation without replay.
async fn verify_completed(
    executor: &ShamirAdminExecutor,
    db: &str,
    operation: &SchemaMigrationOp,
    progress: &OperationProgress,
) -> Result<String, BatchError> {
    let observed = Box::pin(verify_postcondition(
        executor,
        db,
        operation,
        true,
        progress.intent.as_deref(),
    ))
    .await?;
    if admission::is_function_family(operation)
        && progress.completion.as_deref() != Some(observed.as_str())
    {
        return Err(coded(
            "schema_migration_postcondition_failed",
            "function-family completion evidence no longer matches".to_string(),
        ));
    }
    Ok(observed)
}

/// Resume reconcile of ONE operation index (design 4.2 restart cases).
async fn resume_operation(
    executor: &ShamirAdminExecutor,
    db: &str,
    journal: &JournalRef<'_>,
    operation: &SchemaMigrationOp,
    index: u32,
    progress: &OperationProgress,
) -> Result<String, BatchError> {
    let intent_evidence = progress.intent.as_deref();
    match progress.state {
        JournalOpState::Completed => Box::pin(verify_completed(executor, db, operation, progress))
            .await
            .map_err(|error| {
                if admission::is_function_family(operation) && access_denied(&error) {
                    return error;
                }
                coded(
                    "schema_migration_postcondition_failed",
                    format!("operation {index} is durably completed but its postcondition no longer holds: {error}"),
                )
            }),
        JournalOpState::IntentPending => {
            let evidence = match Box::pin(reconcile_pending(
                executor,
                db,
                journal,
                operation,
                index,
                intent_evidence,
            ))
            .await?
            {
                ReconcileOutcome::AlreadyApplied(evidence) => evidence,
                ReconcileOutcome::ExecuteOnce(created_from_absent) => {
                    if let Err(error) = Box::pin(execute_operation(executor, operation)).await {
                        let in_doubt = Box::pin(record_in_doubt(
                            executor,
                            journal,
                            index,
                            format!(
                                "operation {index} failed while finishing a resume execution: {error}"
                            ),
                        ))
                        .await;
                        return Err(preserve_family_denial(operation, error, in_doubt));
                    }
                    match Box::pin(verify_postcondition(
                        executor,
                        db,
                        operation,
                        created_from_absent,
                        intent_evidence,
                    ))
                    .await
                    {
                        Ok(evidence) => evidence,
                        Err(error) => {
                            let failure = Box::pin(record_postcondition_failure(
                                executor,
                                journal,
                                index,
                                format!(
                                    "operation {index} postcondition failed during resume: {error}"
                                ),
                            ))
                            .await;
                            return Err(preserve_family_denial(operation, error, failure));
                        }
                    }
                }
            };
            Box::pin(append_step(
                executor,
                journal,
                SchemaMigrationStep {
                    kind: JournalEventKind::OperationComplete,
                    operation_index: Some(index),
                    evidence: Some(evidence.clone()),
                },
            ))
            .await
            .map_err(|error| durability_error(error, true))?;
            Ok(evidence)
        }
        JournalOpState::NotStarted => Err(plain("cannot resume an unstarted operation".to_string())),
    }
}

/// The normal path of ONE operation: intent, one bounded DDL, exact
/// postcondition, completion (design 4.1).
async fn run_operation(
    executor: &ShamirAdminExecutor,
    db: &str,
    journal: &JournalRef<'_>,
    operation: &SchemaMigrationOp,
    index: u32,
    intents_durable: &mut bool,
) -> Result<(Option<String>, String), BatchError> {
    let ordered_check = matches!(operation, SchemaMigrationOp::CheckFunction(_));
    if ordered_check {
        Box::pin(execute_operation(executor, operation)).await?;
    }
    // Create postconditions need the pre-image: a pre-existing object is a
    // conflict (4.2), never an `if_not_exists` success. The pre-image is
    // recorded in the intent evidence so a later resume can decide the
    // restart case (R1c step 2b-3b).
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
    let intent_evidence = match operation {
        SchemaMigrationOp::CreateTable(_) | SchemaMigrationOp::CreateIndex(_) => {
            Some(if created_from_absent {
                "pre_state=absent".to_string()
            } else {
                "pre_state=present".to_string()
            })
        }
        _ if admission::is_function_family(operation) => {
            Some(Box::pin(reconcile::capture_intent(executor, operation)).await?)
        }
        _ => None,
    };

    // Persist + sync the operation intent BEFORE any effect (4.1). After the
    // first durable intent a later journal failure means an effect may
    // already have occurred: in doubt, never plain unavailability.
    Box::pin(append_step(
        executor,
        journal,
        SchemaMigrationStep {
            kind: JournalEventKind::OperationIntent,
            operation_index: Some(index),
            evidence: intent_evidence.clone(),
        },
    ))
    .await
    .map_err(|error| durability_error(error, *intents_durable))?;
    *intents_durable = true;

    // Execute exactly one bounded DDL operation through the EXISTING handler
    // paths: existing validation, per-target authorization and HMAC gates
    // apply unchanged.
    let execution = if ordered_check {
        Ok(())
    } else {
        Box::pin(execute_operation(executor, operation)).await
    };
    if let Err(error) = execution {
        let in_doubt = Box::pin(record_in_doubt(
            executor,
            journal,
            index,
            format!("operation {index} failed before its postcondition: {error}"),
        ))
        .await;
        return Err(preserve_family_denial(operation, error, in_doubt));
    }

    // Verify the exact postcondition; a failure records no completion.
    let evidence = match Box::pin(verify_postcondition(
        executor,
        db,
        operation,
        created_from_absent,
        intent_evidence.as_deref(),
    ))
    .await
    {
        Ok(evidence) => evidence,
        Err(error) => {
            let failure = Box::pin(record_postcondition_failure(
                executor,
                journal,
                index,
                format!("operation {index} postcondition failed: {error}"),
            ))
            .await;
            return Err(preserve_family_denial(operation, error, failure));
        }
    };

    // Persist + sync the operation completion only after the observed
    // postcondition (never record complete before it is observed).
    Box::pin(append_step(
        executor,
        journal,
        SchemaMigrationStep {
            kind: JournalEventKind::OperationComplete,
            operation_index: Some(index),
            evidence: Some(evidence.clone()),
        },
    ))
    .await
    .map_err(|error| durability_error(error, *intents_durable))?;
    Ok((intent_evidence, evidence))
}

fn access_denied(error: &BatchError) -> bool {
    matches!(error, BatchError::QueryError { code: Some(code), .. } if code == "access_denied")
}

fn preserve_family_denial(
    operation: &SchemaMigrationOp,
    error: BatchError,
    fallback: BatchError,
) -> BatchError {
    if admission::is_function_family(operation) && access_denied(&error) {
        error
    } else {
        fallback
    }
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
        _ => {
            // Admission also rejects bind. Keep replay fail-closed even if an
            // intent was written by older code or constructed directly.
            admission::validate_binding_scope(&executor.db_name, operation)?;
            admission::defer_bind(operation)?;
            // A replay is still an execution and must check the actual
            // source ACL, not the destination authorization used for proof.
            Box::pin(admission::authorize_family_operation(executor, operation)).await?;
            let op = admission::family_batch_op(operation)
                .ok_or_else(|| plain("unsupported migration operation".to_string()))?;
            Box::pin(execution::execute(executor, &op)).await?;
        }
    }
    Ok(())
}

// ── resume reconcile (§4.2 restart cases, R1c step 2b-3b, DDL subset) ──────

/// How far the previous runner got for one operation index, read from the
/// durable journal event rows.
#[derive(Clone, Copy, Default)]
enum JournalOpState {
    /// No intent, no completion: run through the normal path.
    #[default]
    NotStarted,
    /// Durable intent without completion (§4.2 case 2).
    IntentPending,
    /// Durable OperationComplete: never re-run, only re-verify.
    Completed,
}

#[derive(Clone, Default)]
struct OperationProgress {
    state: JournalOpState,
    intent: Option<String>,
    completion: Option<String>,
}

/// What the reconcile pass decided for one pending intent.
enum ReconcileOutcome {
    /// The desired state is proven durable: append the completion with this
    /// evidence WITHOUT replaying the DDL.
    AlreadyApplied(String),
    /// Proven not taken effect (or provably idempotent): execute exactly
    /// once under the new fence; the bool is `created_from_absent` for the
    /// create postconditions.
    ExecuteOnce(bool),
}

/// Classify every operation index of the submitted body from the durable
/// journal rows of this migration. Its own function, not part of the
/// per-operation future (keeps the runner's state machine small).
async fn classify_resume(
    executor: &ShamirAdminExecutor,
    serving: DatabaseIdentity,
    metadata: &SchemaMigrationMetadata,
    checksum: [u8; 32],
    operation_count: usize,
) -> Result<Vec<OperationProgress>, BatchError> {
    let history = executor
        .shamir
        .system_store()
        .read_schema_migration_history(serving)
        .await
        .map_err(map_store_error)?;
    let mut states = vec![OperationProgress::default(); operation_count];
    for event in &history.events {
        if event.migration.id != metadata.id || event.checksum != checksum {
            continue;
        }
        let Some(index) = event.operation_index else {
            continue;
        };
        let position = usize::try_from(index).map_err(|_| {
            coded(
                "schema_migration_gap",
                "operation index overflow".to_string(),
            )
        })?;
        let Some(state) = states.get_mut(position) else {
            return Err(coded(
                "schema_migration_gap",
                format!("journal records operation index {index} beyond the migration body"),
            ));
        };
        match event.kind {
            JournalEventKind::OperationIntent => {
                state.state = JournalOpState::IntentPending;
                state.intent.clone_from(&event.evidence);
                state.completion = None;
            }
            JournalEventKind::OperationComplete => {
                state.state = JournalOpState::Completed;
                state.completion.clone_from(&event.evidence);
            }
            _ => {}
        }
    }
    Ok(states)
}

/// Reconcile decision table for ONE durable intent without completion
/// (§4.2 case 2), per operation kind. Every decision is proven from the
/// stored objects, never guessed; anything unprovable is recorded in doubt
/// (blocking every later operation) instead:
///
/// | kind              | observed state                          | decision |
/// |-------------------|-----------------------------------------|----------|
/// | Create table/index| intent `pre_state=absent`, still absent | execute once, verify, complete |
/// | Create table/index| intent `pre_state=absent`, now present  | the fence blocked every other writer since the intent, so the object is this operation's effect: the normal postcondition verifies → complete WITHOUT replay; it does not hold → in doubt |
/// | Create table/index| intent `pre_state=present`              | absent → execute once; present → in doubt (conflict) |
/// | Create table/index| no recorded pre-state (legacy intent)   | in doubt (never guess) |
/// | Drop table/index  | absent                                  | complete WITHOUT replay |
/// | Drop table/index  | present                                 | execute once (proven not applied), verify, complete |
/// | Rename table/index| source absent AND destination present   | complete WITHOUT replay (proven applied) |
/// | Rename table/index| source present AND destination absent   | execute once, verify, complete |
/// | Rename table/index| both present or both absent             | in doubt, naming the operation index |
/// | SetTableSchema    | stored rules already equal the target   | complete WITHOUT replay |
/// | SetTableSchema    | not equal                               | execute once — exact-equality verification makes one execution idempotent, so execute-once is chosen over guessing |
/// | AddSchemaRule     | rule contained                          | complete WITHOUT replay |
/// | AddSchemaRule     | rule absent                             | execute once |
/// | RemoveSchemaRule  | path absent                             | complete WITHOUT replay |
/// | RemoveSchemaRule  | path present                            | execute once |
/// | Function/validator registration | exact target effect proven against intent | complete without replay |
/// | Function/validator registration | exact pre-state still holds | execute once through normal authorization |
/// | Rename | source absent, destination matches source fingerprint and destination write is authorized | complete without replay |
/// | Binding | exact binding state (bind execution remains deferred) | reconcile without guessing |
/// | CheckFunction | no catalogue effect | rerun read-only check if pending |
///
/// A missing rename source cannot authorize an execution. Reconciliation
/// can only accept the proven destination effect, with destination authority;
/// if it must execute the rename, normal source authorization still applies.
async fn reconcile_pending(
    executor: &ShamirAdminExecutor,
    db: &str,
    journal: &JournalRef<'_>,
    operation: &SchemaMigrationOp,
    index: u32,
    intent_evidence: Option<&str>,
) -> Result<ReconcileOutcome, BatchError> {
    let doubt = |reason: String| {
        record_in_doubt(
            executor,
            journal,
            index,
            format!("operation {index} is unresolved after restart: {reason}"),
        )
    };
    match operation {
        SchemaMigrationOp::CreateTable(op) => match intent_evidence {
            Some("pre_state=absent") => {
                if table_exists(&executor.shamir, db, &op.repo, &op.create_table) {
                    // Absent at intent, and the maintenance fence blocked every
                    // other writer since: the table is this operation's effect.
                    // The same postcondition as a normal completion decides.
                    return match Box::pin(ddl_postconditions::verify_postcondition(executor, db, operation, true))
                        .await
                    {
                        Ok(evidence) => Ok(ReconcileOutcome::AlreadyApplied(evidence)),
                        Err(error) => Err(doubt(format!(
                            "create table '{}/{}': the table now exists but its postcondition does not hold: {error}",
                            op.repo, op.create_table
                        ))
                        .await),
                    };
                }
                Ok(ReconcileOutcome::ExecuteOnce(true))
            }
            Some("pre_state=present") => {
                if table_exists(&executor.shamir, db, &op.repo, &op.create_table) {
                    return Err(doubt(format!(
                        "create table '{}/{}': the pre-state recorded a conflicting \
                         pre-existing table and it is still present",
                        op.repo, op.create_table
                    ))
                    .await);
                }
                Ok(ReconcileOutcome::ExecuteOnce(true))
            }
            _ => Err(doubt(
                "create intent carries no recorded pre-state (legacy journal); \
                 refusing to guess"
                    .to_string(),
            )
            .await),
        },
        SchemaMigrationOp::CreateIndex(op) => match intent_evidence {
            Some("pre_state=absent") => {
                if Box::pin(index_exists(
                    executor,
                    &op.repo,
                    &op.table,
                    &op.create_index,
                ))
                .await
                {
                    // Absent at intent, writers fenced since: the index is this
                    // operation's effect. The same postcondition as a normal
                    // completion decides.
                    return match Box::pin(ddl_postconditions::verify_postcondition(executor, db, operation, true))
                        .await
                    {
                        Ok(evidence) => Ok(ReconcileOutcome::AlreadyApplied(evidence)),
                        Err(error) => Err(doubt(format!(
                            "create index '{}' on '{}/{}': the index now exists but its postcondition does not hold: {error}",
                            op.create_index, op.repo, op.table
                        ))
                        .await),
                    };
                }
                Ok(ReconcileOutcome::ExecuteOnce(true))
            }
            Some("pre_state=present") => {
                if Box::pin(index_exists(
                    executor,
                    &op.repo,
                    &op.table,
                    &op.create_index,
                ))
                .await
                {
                    return Err(doubt(format!(
                        "create index '{}' on '{}/{}': the pre-state recorded a \
                         conflicting pre-existing index and it is still present",
                        op.create_index, op.repo, op.table
                    ))
                    .await);
                }
                Ok(ReconcileOutcome::ExecuteOnce(true))
            }
            _ => Err(doubt(
                "create intent carries no recorded pre-state (legacy journal); \
                 refusing to guess"
                    .to_string(),
            )
            .await),
        },
        SchemaMigrationOp::DropTable(op) => {
            if !table_exists(&executor.shamir, db, &op.repo, &op.drop_table) {
                Ok(ReconcileOutcome::AlreadyApplied(format!(
                    "dropped table '{}/{}'",
                    op.repo, op.drop_table
                )))
            } else {
                Ok(ReconcileOutcome::ExecuteOnce(false))
            }
        }
        SchemaMigrationOp::DropIndex(op) => {
            if !Box::pin(index_exists(executor, &op.repo, &op.table, &op.drop_index)).await {
                Ok(ReconcileOutcome::AlreadyApplied(format!(
                    "dropped index '{}' on '{}/{}'",
                    op.drop_index, op.repo, op.table
                )))
            } else {
                Ok(ReconcileOutcome::ExecuteOnce(false))
            }
        }
        SchemaMigrationOp::RenameTable(op) => {
            let source = table_exists(&executor.shamir, db, &op.repo, &op.rename_table);
            let destination = table_exists(&executor.shamir, db, &op.repo, &op.to);
            match (source, destination) {
                (false, true) => Ok(ReconcileOutcome::AlreadyApplied(format!(
                    "renamed table '{}/{}' → '{}'",
                    op.repo, op.rename_table, op.to
                ))),
                (true, false) => Ok(ReconcileOutcome::ExecuteOnce(false)),
                _ => Err(doubt(format!(
                    "rename table '{}/{}' → '{}': source and destination are in an \
                     ambiguous state after restart",
                    op.repo, op.rename_table, op.to
                ))
                .await),
            }
        }
        SchemaMigrationOp::RenameIndex(op) => {
            let source = Box::pin(index_exists(
                executor,
                &op.repo,
                &op.table,
                &op.rename_index,
            ))
            .await;
            let destination = Box::pin(index_exists(executor, &op.repo, &op.table, &op.to)).await;
            match (source, destination) {
                (false, true) => Ok(ReconcileOutcome::AlreadyApplied(format!(
                    "renamed index '{}' → '{}' on '{}/{}'",
                    op.rename_index, op.to, op.repo, op.table
                ))),
                (true, false) => Ok(ReconcileOutcome::ExecuteOnce(false)),
                _ => Err(doubt(format!(
                    "rename index '{}' → '{}' on '{}/{}': source and destination are in \
                     an ambiguous state after restart",
                    op.rename_index, op.to, op.repo, op.table
                ))
                .await),
            }
        }
        SchemaMigrationOp::SetTableSchema(op) => {
            let rules = stored_rules(executor, &op.repo, &op.set_table_schema).await?;
            if rules == op.schema {
                Ok(ReconcileOutcome::AlreadyApplied(format!(
                    "set schema of table '{}/{}' ({} rules)",
                    op.repo,
                    op.set_table_schema,
                    rules.len()
                )))
            } else {
                // One execution is idempotent under the exact-equality
                // postcondition, so execute-once beats guessing (documented).
                Ok(ReconcileOutcome::ExecuteOnce(false))
            }
        }
        SchemaMigrationOp::AddSchemaRule(op) => {
            let rules = stored_rules(executor, &op.repo, &op.add_schema_rule).await?;
            if rules.iter().any(|rule| rule == &op.rule) {
                Ok(ReconcileOutcome::AlreadyApplied(format!(
                    "added schema rule to table '{}/{}'",
                    op.repo, op.add_schema_rule
                )))
            } else {
                Ok(ReconcileOutcome::ExecuteOnce(false))
            }
        }
        SchemaMigrationOp::RemoveSchemaRule(op) => {
            let rules = stored_rules(executor, &op.repo, &op.remove_schema_rule).await?;
            if !rules.iter().any(|rule| rule.path == op.path) {
                Ok(ReconcileOutcome::AlreadyApplied(format!(
                    "removed schema rule from table '{}/{}'",
                    op.repo, op.remove_schema_rule
                )))
            } else {
                Ok(ReconcileOutcome::ExecuteOnce(false))
            }
        }
        _ => match Box::pin(reconcile::reconcile(executor, operation, intent_evidence)).await {
            Ok(Some(evidence)) => Ok(ReconcileOutcome::AlreadyApplied(evidence)),
            Ok(None) => Ok(ReconcileOutcome::ExecuteOnce(true)),
            Err(error) => {
                let in_doubt = Box::pin(doubt(error.to_string())).await;
                Err(if access_denied(&error) {
                    error
                } else {
                    in_doubt
                })
            }
        },
    }
}

/// Family verification uses the original intent (not a new catalogue read
/// of the pre-state). In particular a rename needs the original fingerprint.
async fn verify_postcondition(
    executor: &ShamirAdminExecutor,
    db: &str,
    operation: &SchemaMigrationOp,
    created_from_absent: bool,
    intent: Option<&str>,
) -> Result<String, BatchError> {
    if admission::is_function_family(operation) {
        Box::pin(reconcile::verify(executor, operation, intent)).await
    } else {
        Box::pin(ddl_postconditions::verify_postcondition(
            executor,
            db,
            operation,
            created_from_absent,
        ))
        .await
    }
}
