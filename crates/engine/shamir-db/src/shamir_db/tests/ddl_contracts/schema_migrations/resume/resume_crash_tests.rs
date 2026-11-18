//! Server-side tests for RESUME and RECONCILE of an interrupted
//! application-schema APPLY (R1c step 2b-3b). Every crash is simulated
//! deterministically through the step-2a durability-tail fault seam
//! (`arm_durability_tail_faults_after(skip, n)`): each journal append spends
//! two fault-gate ticks — one in the pre-transaction journal-tail flush and
//! one in the post-commit durable flush — so append `k` of a run occupies
//! ticks `2k-1` (pre) and `2k` (post). The per-run append order is
//! acquire(1), intent(2), complete(3), applied(4), release(5), and arming
//! `skip = 2k-2, n = 1` fails the run exactly BEFORE append `k` is written.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::SchemaMigration;
use shamir_query_types::admin::types::schema_migration::SchemaMigrationBody;

use super::helpers::{
    apply_request, assert_code, completions_for, empty_predecessor, field, identity, journal_kinds,
    journal_prefix, persistent_owner_db, persistent_owner_db_fjall_repo, reopen_owner_db,
    run_apply, str_field, submission,
};
use crate::shamir_db::system_store::JournalEventKind;
use crate::shamir_db::ShamirDb;

async fn create_table(db: &ShamirDb, name: &str) {
    let mut b = Batch::new();
    b.op("ct", ddl::create_table(name).build());
    let request = b.to_request_via_msgpack().unwrap();
    db.execute_as(super::helpers::OWNER, "testdb", &request)
        .await
        .unwrap();
}

/// A two-operation body: create a table, then an index on it.
fn multi_op_body() -> SchemaMigrationBody {
    let create = ddl::create_table("items_1").build();
    let index = ddl::create_index("ix_items_name", "items_1")
        .field("name")
        .build();
    SchemaMigration::new("alpha-001", 1, "multi operation migration")
        .op(create)
        .expect("typed create-table operation is admitted")
        .op(index)
        .expect("typed create-index operation is admitted")
        .build()
        .expect("test migration body is valid")
}

/// Rename a table, then create another one (a later operation whose blocking
/// proves the in-doubt fence stops the whole run).
fn rename_then_create_body() -> SchemaMigrationBody {
    let rename = ddl::rename_table("items_1", "renamed_items").build();
    let create = ddl::create_table("after_rename").build();
    SchemaMigration::new("alpha-001", 1, "rename then create")
        .op(rename)
        .expect("typed rename-table operation is admitted")
        .op(create)
        .expect("typed create-table operation is admitted")
        .build()
        .expect("test migration body is valid")
}

#[tokio::test]
async fn crash_after_started_resumes_and_runs_first_operation() {
    let (dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = super::super::helpers::body("alpha-001", 1);
    let request = apply_request(&target, empty_predecessor(), 0, submission(&migration));

    // Crash after `started` with ZERO operations attempted: fail the FIRST
    // operation-intent append before it is written (append 2 → tick 3).
    db.system_store().arm_durability_tail_faults_after(2, 1);
    let error = run_apply(&db, request)
        .await
        .expect_err("the failed intent append must stop the run");
    assert_code(error, "schema_migration_durability_unavailable");
    assert_eq!(
        journal_kinds(&db).await,
        vec![JournalEventKind::Started],
        "the journal holds only the started event"
    );

    // Reopen over the same Fjall directory: the journal is unresolved, the
    // control record still owns fence generation 1.
    drop(db);
    let db = reopen_owner_db(&dir).await;
    let target = identity(&db).await;
    let value = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("an identical submission resumes the unresolved migration");

    // The resumed result has the shape of a normal apply's result, reporting
    // the NEW (recovery) fence generation 2.
    assert_eq!(str_field(&value, "status"), "applied");
    assert_eq!(str_field(&value, "migration_id"), "alpha-001");
    assert_eq!(field(&value, "ordinal").as_i64(), Some(1));
    assert_eq!(field(&value, "fence_generation").as_i64(), Some(2));
    assert!(value.get("applied_at_unix_micros").is_some());
    assert!(value.get("checksum").is_some());

    // Operation 1 ran once through the normal path under the new fence.
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(
        journal_kinds(&db).await,
        vec![
            JournalEventKind::Started,
            JournalEventKind::RecoveryAcquired,
            JournalEventKind::OperationIntent,
            JournalEventKind::OperationComplete,
            JournalEventKind::Applied,
            JournalEventKind::FenceReleased,
        ],
        "resumed run: {history:?}"
    );
    assert_eq!(completions_for(&history, 0), 1);
    assert_eq!(history.applied.len(), 1);
    assert_eq!(history.applied[0].fence_generation, 2);
    assert!(history.unresolved.is_empty());
    assert!(db.get_db("testdb").unwrap().has_table("main", "items_1"));
}

#[tokio::test]
async fn crash_after_intent_before_ddl_resumes_once() {
    // The FILE-BACKED repo fixture: this test's pre/post-restart assertions
    // are about the physical table, so the repo must survive the reopen
    // (an in-memory repo would be re-created empty and make them vacuous).
    let (dir, db) = persistent_owner_db_fjall_repo().await;
    let migration = super::super::helpers::body("alpha-001", 1);

    // Crash built directly in the journal (see `journal_prefix`): the crash
    // point must not depend on how many durable tails a DDL effect spends.
    // Crash AFTER the operation-0 intent is durable but BEFORE the DDL
    // effect ran — the window between `crash_after_started…` (started only)
    // and the post-effect windows. The intent records the pre-state evidence
    // a live apply would have written (`pre_state=absent`): a pending create
    // without it refuses as uncertain (apply_operations.rs reconcile table),
    // which is a different scenario.
    journal_prefix(
        &db,
        &migration,
        &[(
            JournalEventKind::OperationIntent,
            0,
            Some("pre_state=absent"),
        )],
    )
    .await;
    let instance = db.get_db("testdb").unwrap();
    assert!(
        !instance.has_table("main", "items_1"),
        "the crash window is before the DDL: the effect has not run"
    );
    // The owned DbInstance clone keeps repository handles alive: release it
    // before the reopen or the Windows lock retry spins to its cap.
    drop(instance);
    drop(db);

    let db = reopen_owner_db(&dir).await;
    let target = identity(&db).await;
    let value = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("an intent without an effect resumes by RUNNING the operation once");
    assert_eq!(str_field(&value, "status"), "applied");

    // The operation ran exactly ONCE through the normal path under the
    // recovery fence: the resume path (`resume_operation` ExecuteOnce)
    // appends the completion WITHOUT a second intent, then applied,
    // released — the durable completion sequence is the replay guard.
    assert_eq!(
        journal_kinds(&db).await,
        vec![
            JournalEventKind::Started,
            JournalEventKind::OperationIntent,
            JournalEventKind::RecoveryAcquired,
            JournalEventKind::OperationComplete,
            JournalEventKind::Applied,
            JournalEventKind::FenceReleased,
        ],
    );
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(completions_for(&history, 0), 1);
    assert_eq!(history.applied.len(), 1);
    assert!(db.get_db("testdb").unwrap().has_table("main", "items_1"));
}

#[tokio::test]
async fn crash_after_create_effect_before_completion_resumes_without_replaying() {
    let (dir, db) = persistent_owner_db().await;
    let migration = super::super::helpers::body("alpha-001", 1);

    // Crash built directly in the journal (see `journal_prefix`): the crash
    // point must not depend on how many durable tails a DDL effect spends.
    // Crash after the DDL effect but before the completion append: the intent
    // recorded `pre_state=absent` and the table exists. The fence blocked every
    // other writer since, so the table is the effect of this operation.
    journal_prefix(
        &db,
        &migration,
        &[(
            JournalEventKind::OperationIntent,
            0,
            Some("pre_state=absent"),
        )],
    )
    .await;
    create_table(&db, "items_1").await;
    assert!(db.get_db("testdb").unwrap().has_table("main", "items_1"));
    drop(db);

    // A replay would create a table that already exists and fail, so an Ok
    // resume with exactly one completion proves the effect was NOT replayed.
    let db = reopen_owner_db(&dir).await;
    let target = identity(&db).await;
    let value = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("absent at intent and present now proves the create applied");
    assert_eq!(str_field(&value, "status"), "applied");
    assert_eq!(field(&value, "fence_generation").as_i64(), Some(2));

    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(completions_for(&history, 0), 1);
    assert!(history.unresolved.is_empty());
    assert_eq!(history.applied.len(), 1);
    let completion = history
        .events
        .iter()
        .find(|event| event.kind == JournalEventKind::OperationComplete)
        .unwrap();
    assert!(
        completion
            .evidence
            .as_deref()
            .is_some_and(|evidence| evidence.contains("created table")),
        "the reconcile completion carries the proven effect: {completion:?}"
    );
    assert!(db.get_db("testdb").unwrap().has_table("main", "items_1"));
}

#[tokio::test]
async fn crash_after_index_effect_before_completion_resumes_without_replaying() {
    let (dir, db) = persistent_owner_db().await;
    create_table(&db, "items_1").await;
    let migration = SchemaMigration::new("alpha-001", 1, "index migration")
        .op(ddl::create_index("ix_items_name", "items_1")
            .field("name")
            .build())
        .expect("typed create-index operation is admitted")
        .build()
        .expect("test migration body is valid");

    // Crash built directly in the journal (see `journal_prefix`): the crash
    // point must not depend on how many durable tails a DDL effect spends.
    // Crash after the index exists but before the completion append; the
    // intent recorded `pre_state=absent`.
    journal_prefix(
        &db,
        &migration,
        &[(
            JournalEventKind::OperationIntent,
            0,
            Some("pre_state=absent"),
        )],
    )
    .await;
    let mut effect = Batch::new();
    effect.op(
        "ix",
        ddl::create_index("ix_items_name", "items_1")
            .field("name")
            .build(),
    );
    db.execute_as(
        super::helpers::OWNER,
        "testdb",
        &effect.to_request_via_msgpack().unwrap(),
    )
    .await
    .unwrap();
    drop(db);

    // A replay would create an index that already exists and fail, so an Ok
    // resume with exactly one completion proves the effect was NOT replayed.
    let db = reopen_owner_db(&dir).await;
    let target = identity(&db).await;
    let value = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("absent at intent and present now proves the index create applied");
    assert_eq!(str_field(&value, "status"), "applied");

    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(completions_for(&history, 0), 1);
    assert!(history.unresolved.is_empty());
    let completion = history
        .events
        .iter()
        .find(|event| event.kind == JournalEventKind::OperationComplete)
        .unwrap();
    assert!(
        completion
            .evidence
            .as_deref()
            .is_some_and(|evidence| evidence.contains("created index")),
        "the reconcile completion carries the proven effect: {completion:?}"
    );
}

#[tokio::test]
async fn crash_after_rename_effect_resumes_and_appends_the_completion_without_replaying() {
    let (dir, db) = persistent_owner_db().await;
    create_table(&db, "items_1").await;
    let migration = SchemaMigration::new("alpha-001", 1, "rename migration")
        .op(ddl::rename_table("items_1", "renamed_items").build())
        .expect("typed rename-table operation is admitted")
        .build()
        .expect("test migration body is valid");

    // Crash built directly in the journal (see `journal_prefix`): the crash
    // point must not depend on how many durable tails a DDL effect spends.
    // Crash after the rename effect executed but before its completion append.
    // A replay would rename a source that no longer exists and fail, so an Ok
    // resume with exactly one completion proves the effect was NOT replayed.
    journal_prefix(
        &db,
        &migration,
        &[(JournalEventKind::OperationIntent, 0, None)],
    )
    .await;
    let mut effect = Batch::new();
    effect.op("rn", ddl::rename_table("items_1", "renamed_items").build());
    db.execute_as(
        super::helpers::OWNER,
        "testdb",
        &effect.to_request_via_msgpack().unwrap(),
    )
    .await
    .unwrap();
    let instance = db.get_db("testdb").unwrap();
    assert!(!instance.has_table("main", "items_1"));
    assert!(instance.has_table("main", "renamed_items"));
    drop(db);

    let db = reopen_owner_db(&dir).await;
    let target = identity(&db).await;
    let value = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("source absent + destination present proves the effect durable");
    assert_eq!(str_field(&value, "status"), "applied");
    assert_eq!(field(&value, "fence_generation").as_i64(), Some(2));

    // The completion was appended WITHOUT replay: exactly one completion for
    // index 0, whose evidence is the reconcile proof, and no second DDL.
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(
        journal_kinds(&db).await,
        vec![
            JournalEventKind::Started,
            JournalEventKind::OperationIntent,
            JournalEventKind::RecoveryAcquired,
            JournalEventKind::OperationComplete,
            JournalEventKind::Applied,
            JournalEventKind::FenceReleased,
        ],
        "resumed run: {history:?}"
    );
    assert_eq!(completions_for(&history, 0), 1);
    let completion = history
        .events
        .iter()
        .find(|event| event.kind == JournalEventKind::OperationComplete)
        .unwrap();
    assert!(
        completion
            .evidence
            .as_deref()
            .is_some_and(|evidence| evidence.contains("renamed table")),
        "the reconcile completion carries the proven effect: {completion:?}"
    );
    let instance = db.get_db("testdb").unwrap();
    assert!(!instance.has_table("main", "items_1"));
    assert!(instance.has_table("main", "renamed_items"));
}

#[tokio::test]
async fn crash_between_operations_resumes_completed_op_without_replay_and_runs_the_next() {
    let (dir, db) = persistent_owner_db().await;
    let migration = multi_op_body();

    // Crash built directly in the journal (see `journal_prefix`): the crash
    // point must not depend on how many durable tails a DDL effect spends.
    // Crash before the SECOND intent: operation 0 is durably completed (its
    // table exists), operation 1 was never attempted.
    journal_prefix(
        &db,
        &migration,
        &[
            (
                JournalEventKind::OperationIntent,
                0,
                Some("pre_state=absent"),
            ),
            (
                JournalEventKind::OperationComplete,
                0,
                Some("created table 'main/items_1'"),
            ),
        ],
    )
    .await;
    create_table(&db, "items_1").await;
    drop(db);
    let db = reopen_owner_db(&dir).await;
    let target = identity(&db).await;
    let value = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("the identical submission resumes between the operations");
    assert_eq!(field(&value, "fence_generation").as_i64(), Some(2));

    // Operation 0 was only re-verified (one completion total), operation 1
    // ran normally under the new fence.
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(
        journal_kinds(&db).await,
        vec![
            JournalEventKind::Started,
            JournalEventKind::OperationIntent,
            JournalEventKind::OperationComplete,
            JournalEventKind::RecoveryAcquired,
            JournalEventKind::OperationIntent,
            JournalEventKind::OperationComplete,
            JournalEventKind::Applied,
            JournalEventKind::FenceReleased,
        ],
        "resumed run: {history:?}"
    );
    assert_eq!(completions_for(&history, 0), 1);
    assert_eq!(completions_for(&history, 1), 1);
    let instance = db.get_db("testdb").unwrap();
    assert!(instance.has_table("main", "items_1"));
    assert!(
        instance
            .get_table("main", "items_1")
            .await
            .unwrap()
            .any_index_exists("ix_items_name")
            .await
    );
}

#[tokio::test]
async fn crash_after_last_completion_resumes_by_appending_only_the_applied_event() {
    let (dir, db) = persistent_owner_db().await;
    let migration = super::super::helpers::body("alpha-001", 1);

    // Crash built directly in the journal (see `journal_prefix`): the crash
    // point must not depend on how many durable tails a DDL effect spends.
    // Crash before the `applied` append: every operation is durably
    // completed, only the applied event is missing.
    journal_prefix(
        &db,
        &migration,
        &[
            (
                JournalEventKind::OperationIntent,
                0,
                Some("pre_state=absent"),
            ),
            (
                JournalEventKind::OperationComplete,
                0,
                Some("created table 'main/items_1'"),
            ),
        ],
    )
    .await;
    create_table(&db, "items_1").await;
    assert_eq!(
        journal_kinds(&db).await,
        vec![
            JournalEventKind::Started,
            JournalEventKind::OperationIntent,
            JournalEventKind::OperationComplete,
        ],
    );
    drop(db);
    let db = reopen_owner_db(&dir).await;
    let target = identity(&db).await;
    let value = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("the identical submission resumes the fully completed run");
    assert_eq!(str_field(&value, "status"), "applied");
    assert_eq!(field(&value, "fence_generation").as_i64(), Some(2));

    // The resume delta is exactly the recovery acquisition, the applied
    // event, and the fence release — no completion is re-appended.
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(
        journal_kinds(&db).await,
        vec![
            JournalEventKind::Started,
            JournalEventKind::OperationIntent,
            JournalEventKind::OperationComplete,
            JournalEventKind::RecoveryAcquired,
            JournalEventKind::Applied,
            JournalEventKind::FenceReleased,
        ],
        "resume delta: {history:?}"
    );
    assert_eq!(completions_for(&history, 0), 1);
    assert_eq!(history.applied.len(), 1);
    assert_eq!(history.applied[0].fence_generation, 2);
    assert!(history.unresolved.is_empty());
}

#[tokio::test]
async fn unprovable_rename_stays_in_doubt_and_blocks_later_operations() {
    let (dir, db) = persistent_owner_db().await;
    create_table(&db, "items_1").await;
    let migration = rename_then_create_body();

    // Crash built directly in the journal (see `journal_prefix`): the crash
    // point must not depend on how many durable tails a DDL effect spends.
    // Crash after the rename effect but before its completion: source gone,
    // destination created.
    journal_prefix(
        &db,
        &migration,
        &[(JournalEventKind::OperationIntent, 0, None)],
    )
    .await;
    let mut effect = Batch::new();
    effect.op("rn", ddl::rename_table("items_1", "renamed_items").build());
    db.execute_as(
        super::helpers::OWNER,
        "testdb",
        &effect.to_request_via_msgpack().unwrap(),
    )
    .await
    .unwrap();
    drop(db);
    let db = reopen_owner_db(&dir).await;
    // Externally re-create the SOURCE with its original name (a direct
    // DbInstance call — the batch entry is fenced in-doubt after restart).
    // Now source AND destination exist: the rename outcome is unprovable.
    db.get_db("testdb")
        .unwrap()
        .create_table("main", "items_1")
        .await
        .unwrap();

    let target = identity(&db).await;
    let error = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect_err("a rename with source and destination both present is unprovable");
    let message = error.to_string();
    assert_code(error, "schema_migration_in_doubt");
    assert!(
        message.contains("operation 0"),
        "the refusal must name the ambiguous operation index: {message}"
    );

    // The in-doubt classification blocks every later operation: no
    // completion at all, no applied event.
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(completions_for(&history, 0), 0);
    assert_eq!(completions_for(&history, 1), 0);
    assert!(history.applied.is_empty());

    // An identical resubmission is refused in doubt again. The takeover
    // itself is one durable RecoveryAcquired; it must NOT complete anything.
    // The refusal records the doubt under the fresh generation.
    let error = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect_err("the identical resubmission stays in doubt");
    assert_code(error, "schema_migration_in_doubt");
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(completions_for(&history, 0), 0);
    assert_eq!(completions_for(&history, 1), 0);
    assert!(history.applied.is_empty());
    let recoveries = history
        .events
        .iter()
        .filter(|event| event.kind == JournalEventKind::RecoveryAcquired)
        .count();
    let doubts = history
        .events
        .iter()
        .filter(|event| event.kind == JournalEventKind::InDoubt)
        .count();
    assert_eq!(recoveries, 2, "each resume takes over the fence once");
    assert_eq!(
        doubts, 2,
        "each refused resume records its own marker under its fresh generation"
    );
}

#[tokio::test]
async fn drifted_completed_operation_is_refused_without_replay() {
    let (dir, db) = persistent_owner_db().await;
    let migration = super::super::helpers::body("alpha-001", 1);

    // Crash built directly in the journal (see `journal_prefix`): the crash
    // point must not depend on how many durable tails a DDL effect spends.
    // Crash after operation 0's durable completion, before `applied`.
    journal_prefix(
        &db,
        &migration,
        &[
            (
                JournalEventKind::OperationIntent,
                0,
                Some("pre_state=absent"),
            ),
            (
                JournalEventKind::OperationComplete,
                0,
                Some("created table 'main/items_1'"),
            ),
        ],
    )
    .await;
    create_table(&db, "items_1").await;
    drop(db);
    let db = reopen_owner_db(&dir).await;
    // Externally DROP the completed operation's table (direct DbInstance
    // call): the durable completion no longer matches reality.
    db.get_db("testdb")
        .unwrap()
        .drop_table("main", "items_1")
        .await
        .unwrap();

    let pristine = journal_kinds(&db).await;
    let target = identity(&db).await;
    let error = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect_err("a drifted completed operation must be refused");
    let message = error.to_string();
    assert_code(error, "schema_migration_postcondition_failed");
    assert!(
        message.contains("durably completed"),
        "the refusal must describe the drift: {message}"
    );

    // No re-run and no marker: the only journal growth is the takeover's
    // RecoveryAcquired (the refusal runs after the fence is taken).
    let mut expected = pristine;
    expected.push(JournalEventKind::RecoveryAcquired);
    assert_eq!(
        journal_kinds(&db).await,
        expected,
        "the drift refusal appends no operation event and no marker"
    );
}
