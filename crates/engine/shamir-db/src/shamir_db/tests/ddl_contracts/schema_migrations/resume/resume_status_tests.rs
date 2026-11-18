//! Server-side tests for the STATUS surface of an interrupted application-
//! schema APPLY (R1c step 2b-3b): the `in_flight` record of an unresolved
//! migration carries the in-doubt evidence, a completed migration leaves no
//! `in_flight`, and a resumed apply's result has the fresh-apply shape.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::types::schema_migration::SchemaMigrationBody;

use super::helpers::{
    apply_request, assert_code, crash_started_only_and_reopen, empty_predecessor, field, identity,
    journal_prefix, persistent_owner_db, reopen_owner_db, run_apply, run_status, str_field,
    submission,
};
use crate::shamir_db::system_store::JournalEventKind;

async fn create_table(db: &crate::ShamirDb, name: &str) {
    let mut b = Batch::new();
    b.op("ct", ddl::create_table(name).build());
    let request = b.to_request_via_msgpack().unwrap();
    db.execute_as(super::helpers::OWNER, "testdb", &request)
        .await
        .unwrap();
}

/// A single rename of an externally pre-created table (the chunk-1
/// rename-both-present crash pattern: crash after the effect, before the
/// completion append; the resume attempt then records the InDoubt marker).
fn rename_body() -> SchemaMigrationBody {
    let rename = ddl::rename_table("items_1", "renamed_items").build();
    shamir_query_builder::SchemaMigration::new("alpha-001", 1, "rename migration")
        .op(rename)
        .expect("typed rename-table operation is admitted")
        .build()
        .expect("test migration body is valid")
}

/// The exact reconcile evidence `reconcile_pending` records for a rename
/// whose source AND destination are present (apply_operations.rs): the
/// doubt closure prefixes `operation {index} is unresolved after restart:`.
const RENAME_BOTH_PRESENT_EVIDENCE: &str = "operation 0 is unresolved after restart: rename \
     table 'main/items_1' → 'renamed_items': source and destination are in an ambiguous \
     state after restart";

#[tokio::test]
async fn status_reports_in_doubt_reason_for_unresolved_migration() {
    let (dir, db) = persistent_owner_db().await;
    create_table(&db, "items_1").await;
    let migration = rename_body();

    // Crash after the rename effect but before its completion append, built
    // directly in the journal (see `journal_prefix`): source gone, destination
    // created.
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
    let target = identity(&db).await;
    // Externally re-create the SOURCE (both present → unprovable): this
    // resume attempt is refused and BEST-EFFORT records the InDoubt marker
    // with the reconcile evidence.
    db.get_db("testdb")
        .unwrap()
        .create_table("main", "items_1")
        .await
        .unwrap();
    let error = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect_err("a rename with source and destination both present is unprovable");
    assert_code(error, "schema_migration_in_doubt");

    // STATUS as OWNER surfaces the unresolved migration with the recorded
    // evidence: `unresolved.last()` is the InDoubt status event itself, so
    // `status` is the in-doubt kind string, the fence generation is the
    // refused resume's takeover generation 2 (the crashed run held 1), and
    // `in_doubt_reason` is exactly the marker's evidence. The InDoubt marker names the blocked operation
    // (index 0 — the rename), so `operation_index` is that pending index.
    let value = run_status(&db, &target).await;
    let in_flight = field(&value, "in_flight");
    assert_eq!(str_field(in_flight, "migration_id"), "alpha-001");
    assert_eq!(field(in_flight, "ordinal").as_i64(), Some(1));
    assert_eq!(field(in_flight, "fence_generation").as_i64(), Some(2));
    assert_eq!(str_field(in_flight, "status"), "in_doubt");
    assert_eq!(
        str_field(in_flight, "in_doubt_reason"),
        RENAME_BOTH_PRESENT_EVIDENCE
    );
    assert_eq!(
        field(in_flight, "operation_index").as_i64(),
        Some(0),
        "the in-doubt marker names the blocked operation index"
    );

    // A resumed-then-completed migration leaves the journal fully applied:
    // STATUS has no `in_flight` record at all.
    let (_dir2, db2, migration2) = crash_started_only_and_reopen().await;
    let target2 = identity(&db2).await;
    let value2 = run_apply(
        &db2,
        apply_request(&target2, empty_predecessor(), 0, submission(&migration2)),
    )
    .await
    .expect("the identical submission resumes the unresolved migration");
    assert_eq!(str_field(&value2, "status"), "applied");
    let status2 = run_status(&db2, &target2).await;
    assert!(
        status2.get("in_flight").is_none(),
        "a completed migration has no in-flight record"
    );
    assert_eq!(str_field(&status2, "last_applied_id"), "alpha-001");
}

#[tokio::test]
async fn resumed_result_shape_equals_a_normal_apply() {
    // db1: a FRESH apply of the one-operation create migration.
    let (_dir1, db1) = persistent_owner_db().await;
    let target1 = identity(&db1).await;
    let fresh = run_apply(
        &db1,
        apply_request(
            &target1,
            empty_predecessor(),
            0,
            submission(&super::super::helpers::body("alpha-001", 1)),
        ),
    )
    .await
    .expect("a fresh apply of the migration succeeds");

    // db2: the SAME migration crashed at its first intent append and resumed.
    let (_dir2, db2, migration) = crash_started_only_and_reopen().await;
    let target2 = identity(&db2).await;
    let resumed = run_apply(
        &db2,
        apply_request(&target2, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("the identical submission resumes the unresolved migration");

    // Every wire field matches the fresh apply except the fence generation
    // (fresh acquires generation 1; the resume recovers at the journal's
    // current generation 2) and `applied_at_unix_micros` (a server-derived
    // wall clock, taken at different moments by construction).
    let fresh_map = fresh.as_object().expect("the apply result is a map");
    let resumed_map = resumed.as_object().expect("the apply result is a map");
    let mut keys: Vec<&String> = fresh_map.keys().collect();
    let mut resumed_keys: Vec<&String> = resumed_map.keys().collect();
    keys.sort();
    resumed_keys.sort();
    assert_eq!(
        keys, resumed_keys,
        "both results carry the same wire fields"
    );
    for key in keys {
        if key == "fence_generation" || key == "applied_at_unix_micros" {
            continue;
        }
        assert_eq!(
            fresh_map.get(key),
            resumed_map.get(key),
            "field '{key}' of the resumed result must equal the fresh apply"
        );
    }
    assert_eq!(
        field(&fresh, "fence_generation").as_i64(),
        Some(1),
        "the fresh apply holds the first fence generation"
    );
    assert_eq!(
        field(&resumed, "fence_generation").as_i64(),
        Some(2),
        "the resume recovers at the journal's current generation"
    );
}
