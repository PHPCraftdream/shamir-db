//! The fenced operation runner of the application-schema APPLY arm
//! (release-1 R1c step 2b-3a/2b-3b, design §4.1–§4.3): per-operation journal
//! intent → one bounded DDL → exact postcondition → completion, then
//! end-state verification, the durable `applied` event, and fence release.
//! On a resume run, a reconcile pass first classifies each operation index
//! from the durable journal and finishes what the previous runner already
//! proved, without re-running completed work. Admission lives in `apply.rs`;
//! the journal append / error mapping / wire encoding live in
//! `apply_journal.rs`.

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
    SchemaMigrationBody, SchemaMigrationMetadata, SchemaMigrationOp, SchemaMigrationSubmission,
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
    /// Resume (recovery-fence takeover) run: reconcile from the journal
    /// before executing anything new (R1c step 2b-3b).
    pub(super) resume: bool,
    pub(super) context: &'a SchemaMigrationContext,
}

/// The fenced run: holds the IN-PROCESS gate for the whole run and converts
/// it to the in-doubt marker on any error exit (see `run_operations`), then
/// delegates to `run_operations_fenced`.
pub(super) async fn run_operations(run: FencedRun<'_>) -> Result<QueryResult, BatchError> {
    let shamir = &run.executor.shamir;
    let db = run.db;
    let generation = run.generation;
    // Fence owned durably: hold the in-process gate so EVERY exit path
    // releases it (mirrors the admission entry, which acquired the durable
    // fence before calling this runner).
    shamir.hold_schema_migration_gate(db, generation);
    let result = Box::pin(run_operations_fenced(run)).await;
    if result.is_err() {
        // Every error exit leaves the durable fence OWNED with an unresolved
        // journal (the fence is only released on the success path): keep
        // writers refused with `schema_migration_in_doubt` until a resume or
        // the next open resolves it, instead of ungating after the RAII
        // release of the live gate.
        shamir.mark_schema_migration_gate_in_doubt(db, generation);
    } else {
        shamir.release_schema_migration_gate(db, generation);
    }
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
    let resume_states = if resume {
        Some(
            Box::pin(classify_resume(
                executor,
                serving,
                metadata,
                checksum,
                body.operations.len(),
            ))
            .await?,
        )
    } else {
        None
    };

    for (position, operation) in body.operations.iter().enumerate() {
        let index = u32::try_from(position).map_err(|_| {
            coded(
                "schema_migration_gap",
                "operation index overflow".to_string(),
            )
        })?;

        // Resume reconcile: act on what the journal proves the previous
        // runner already did (never re-run completed work, never conceal
        // drift). An in-doubt classification blocks every later operation.
        // Each step is its own boxed future: a Windows debug test thread has
        // a small stack and this runner state machine must stay small.
        if let Some(states) = &resume_states {
            let (state, intent_evidence) = &states[position];
            if let ResumeStep::Done = Box::pin(resume_operation(
                executor,
                db,
                &journal,
                operation,
                index,
                *state,
                intent_evidence.as_deref(),
            ))
            .await?
            {
                continue;
            }
        }

        Box::pin(run_operation(
            executor,
            db,
            &journal,
            operation,
            index,
            &mut intents_durable,
        ))
        .await?;
    }

    // End-state invariants: every operation's postcondition still holds.
    for operation in &body.operations {
        Box::pin(verify_postcondition(executor, db, operation, true)).await?;
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

/// What the resume pass did for one operation index.
enum ResumeStep {
    /// Completed or reconciled from the journal: nothing left to run.
    Done,
    /// No durable intent: run the operation through the normal path.
    RunNormally,
}

/// Resume reconcile of ONE operation index (design 4.2 restart cases).
async fn resume_operation(
    executor: &ShamirAdminExecutor,
    db: &str,
    journal: &JournalRef<'_>,
    operation: &SchemaMigrationOp,
    index: u32,
    state: JournalOpState,
    intent_evidence: Option<&str>,
) -> Result<ResumeStep, BatchError> {
    match state {
        JournalOpState::Completed => {
            // Durable completion: only re-verify; drift is refused with no
            // write and no re-run.
            if let Err(error) = Box::pin(verify_postcondition(executor, db, operation, true)).await
            {
                return Err(coded(
                    "schema_migration_postcondition_failed",
                    format!(
                        "operation {index} is durably completed but its postcondition no longer holds: {error}"
                    ),
                ));
            }
            Ok(ResumeStep::Done)
        }
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
                        return Err(Box::pin(record_in_doubt(
                            executor,
                            journal,
                            index,
                            format!(
                                "operation {index} failed while finishing a resume execution: {error}"
                            ),
                        ))
                        .await);
                    }
                    match Box::pin(verify_postcondition(
                        executor,
                        db,
                        operation,
                        created_from_absent,
                    ))
                    .await
                    {
                        Ok(evidence) => evidence,
                        Err(error) => {
                            return Err(Box::pin(record_postcondition_failure(
                                executor,
                                journal,
                                index,
                                format!(
                                    "operation {index} postcondition failed during resume: {error}"
                                ),
                            ))
                            .await)
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
                    evidence: Some(evidence),
                },
            ))
            .await
            .map_err(|error| durability_error(error, true))?;
            Ok(ResumeStep::Done)
        }
        JournalOpState::NotStarted => Ok(ResumeStep::RunNormally),
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
) -> Result<(), BatchError> {
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
        _ => None,
    };

    // Persist + sync the operation intent BEFORE the effect (4.1). After the
    // first durable intent a later journal failure means an effect may
    // already have occurred: in doubt, never plain unavailability.
    Box::pin(append_step(
        executor,
        journal,
        SchemaMigrationStep {
            kind: JournalEventKind::OperationIntent,
            operation_index: Some(index),
            evidence: intent_evidence,
        },
    ))
    .await
    .map_err(|error| durability_error(error, *intents_durable))?;
    *intents_durable = true;

    // Execute exactly one bounded DDL operation through the EXISTING handler
    // paths: existing validation, per-target authorization and HMAC gates
    // apply unchanged.
    if let Err(error) = Box::pin(execute_operation(executor, operation)).await {
        // The effect outcome is uncertain: record in doubt (best effort) and
        // stop before the next effect, journal resumable.
        return Err(Box::pin(record_in_doubt(
            executor,
            journal,
            index,
            format!("operation {index} failed before its postcondition: {error}"),
        ))
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
            return Err(Box::pin(record_postcondition_failure(
                executor,
                journal,
                index,
                format!("operation {index} postcondition failed: {error}"),
            ))
            .await);
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
            evidence: Some(evidence),
        },
    ))
    .await
    .map_err(|error| durability_error(error, *intents_durable))?;
    Ok(())
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

// ── resume reconcile (§4.2 restart cases, R1c step 2b-3b, DDL subset) ──────

/// How far the previous runner got for one operation index, read from the
/// durable journal event rows.
#[derive(Clone, Copy)]
enum JournalOpState {
    /// No intent, no completion: run through the normal path.
    NotStarted,
    /// Durable intent without completion (§4.2 case 2). The tuple carries
    /// the intent's recorded evidence (create pre-state, if any).
    IntentPending,
    /// Durable OperationComplete: never re-run, only re-verify.
    Completed,
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
) -> Result<Vec<(JournalOpState, Option<String>)>, BatchError> {
    let history = executor
        .shamir
        .system_store()
        .read_schema_migration_history(serving)
        .await
        .map_err(map_store_error)?;
    let mut states = vec![(JournalOpState::NotStarted, None); operation_count];
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
                *state = (JournalOpState::IntentPending, event.evidence.clone())
            }
            JournalEventKind::OperationComplete => *state = (JournalOpState::Completed, None),
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
                    return match Box::pin(verify_postcondition(executor, db, operation, true))
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
                    return match Box::pin(verify_postcondition(executor, db, operation, true))
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
        // Refused at admission; unreachable but kept total.
        _ => Err(plain(FUNCTION_FAMILY_DEFERRED.to_string())),
    }
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
