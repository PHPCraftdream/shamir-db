//! Server-side tests for the EXECUTION half of application-schema APPLY
//! (R1c step 2b-3a): the fenced, journaled run of a multi-operation
//! migration, the applied-ordinal retry no-op, and the resumable journal
//! left behind by a failed postcondition.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::SchemaMigration;
use shamir_query_types::admin::types::schema_migration::SchemaMigrationBody;

use super::helpers::{
    apply_request, assert_code, empty_predecessor, field, identity, journal_rows,
    persistent_owner_db, run_apply, str_field, submission,
};
use crate::shamir_db::system_store::JournalEventKind;
use crate::ShamirDb;

/// A two-operation migration body: create a table, then an index on it.
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

async fn create_table(db: &ShamirDb, name: &str) {
    let mut b = Batch::new();
    b.op("ct", ddl::create_table(name).build());
    let request = b.to_request_via_msgpack().unwrap();
    db.execute("testdb", &request).await.unwrap();
}

#[tokio::test]
async fn apply_runs_a_multi_operation_migration_journals_the_full_sequence_and_creates_the_objects()
{
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = multi_op_body();

    let value = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("the owner's apply of a valid migration is admitted and executes");

    // The wire result reports the applied migration under the fence.
    assert_eq!(str_field(&value, "status"), "applied");
    assert_eq!(str_field(&value, "migration_id"), "alpha-001");
    assert_eq!(field(&value, "ordinal").as_i64(), Some(1));
    assert_eq!(field(&value, "fence_generation").as_i64(), Some(1));
    assert!(value.get("applied_at_unix_micros").is_some());

    // Journal sequence: started → intent/complete per operation → applied →
    // fence released.
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    let kinds: Vec<JournalEventKind> = history.events.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        vec![
            JournalEventKind::Started,
            JournalEventKind::OperationIntent,
            JournalEventKind::OperationComplete,
            JournalEventKind::OperationIntent,
            JournalEventKind::OperationComplete,
            JournalEventKind::Applied,
            JournalEventKind::FenceReleased,
        ],
        "journal sequence: {kinds:?}"
    );
    assert_eq!(history.events.len(), 7);
    assert_eq!(history.events[0].body.as_ref(), Some(&migration));
    assert_eq!(
        history.events[0].checksum,
        super::super::helpers::checksum(&migration)
    );
    assert_eq!(history.events[1].operation_index, Some(0));
    assert_eq!(history.events[3].operation_index, Some(1));
    assert_eq!(history.applied.len(), 1);
    assert_eq!(history.applied[0].fence_generation, 1);
    assert!(history.unresolved.is_empty());
    assert!(!history.in_doubt);

    // The objects exist: the created table and its created index.
    let instance = db.get_db("testdb").unwrap();
    assert!(instance.has_table("main", "items_1"));
    let table = instance.get_table("main", "items_1").await.unwrap();
    assert!(table.any_index_exists("ix_items_name").await);

    // The in-process gate was released with the durable fence.
    assert!(db.schema_migration_gate_generation("testdb").is_none());
}

#[tokio::test]
async fn retry_of_an_applied_migration_is_a_noop_returning_the_stored_applied_result() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = super::super::helpers::body("alpha-001", 1);
    let request = apply_request(&target, empty_predecessor(), 0, submission(&migration));

    let first = run_apply(&db, request.clone())
        .await
        .expect("the first apply executes the migration");
    assert_eq!(str_field(&first, "status"), "applied");

    let before = journal_rows(&db).await;
    let second = run_apply(&db, request)
        .await
        .expect("the retry is admitted as a durable no-op");

    // The no-op result echoes the STORED applied outcome: same status,
    // applied time, fence generation and checksum as the first run.
    for key in [
        "status",
        "migration_id",
        "applied_at_unix_micros",
        "fence_generation",
        "checksum",
    ] {
        assert_eq!(field(&first, key), field(&second, key), "field {key}");
    }

    // And it wrote nothing: the journal is byte-identical.
    assert_eq!(journal_rows(&db).await, before, "no-op must write nothing");
}

#[tokio::test]
async fn a_changed_body_for_an_applied_ordinal_is_refused_without_a_write() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let applied = super::super::helpers::body("alpha-001", 1);
    run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&applied)),
    )
    .await
    .expect("the original migration applies");

    // Same id and ordinal, a different body, and a SELF-CONSISTENT checksum:
    // only the durable applied prefix can expose the drift.
    let redefined = SchemaMigration::new("alpha-001", 1, "migration 1 redefined")
        .op(ddl::create_table("different_table").build())
        .expect("typed create-table operation is admitted")
        .build()
        .expect("test migration body is valid");

    let before = journal_rows(&db).await;
    let error = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&redefined)),
    )
    .await
    .expect_err("a redefined applied ordinal must be refused");
    assert_code(error, "schema_migration_checksum_mismatch");
    assert_eq!(
        journal_rows(&db).await,
        before,
        "the refusal writes nothing"
    );
}

#[tokio::test]
async fn a_failed_postcondition_leaves_started_and_intent_without_completion_and_is_reported() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;

    // The table already exists, so the create operation has a conflicting
    // pre-image; the handler's `if_not_exists` succeeds silently, and the
    // exact postcondition (created from absent) must fail.
    create_table(&db, "items_1").await;
    let migration = SchemaMigration::new("alpha-001", 1, "conflicting create")
        .op(ddl::create_table("items_1").if_not_exists().build())
        .expect("typed create-table operation is admitted")
        .build()
        .expect("test migration body is valid");

    let error = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect_err("a conflicting create must fail its postcondition");
    assert_code(error, "schema_migration_postcondition_failed");

    // The journal is left resumable: started + intent for the operation,
    // an in-doubt marker with the postcondition evidence, and NO completion.
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    let kinds: Vec<JournalEventKind> = history.events.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        vec![
            JournalEventKind::Started,
            JournalEventKind::OperationIntent,
            JournalEventKind::InDoubt,
        ],
        "resumable journal: {kinds:?}"
    );
    assert!(!history
        .events
        .iter()
        .any(|event| event.kind == JournalEventKind::OperationComplete));
    assert!(history.applied.is_empty());
    assert!(
        history
            .events
            .last()
            .unwrap()
            .evidence
            .as_deref()
            .is_some_and(|evidence| evidence.contains("postcondition failed")),
        "the in-doubt evidence must name the failed postcondition"
    );
}
