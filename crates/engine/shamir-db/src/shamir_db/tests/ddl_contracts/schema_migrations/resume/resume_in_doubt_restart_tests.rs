//! Restart with a durable IN-DOUBT journal tail already present (design
//! §4.2; brief item "restarted in-doubt tail"): the FIRST boot after the
//! crash asserts the in-doubt evidence from STATUS alone — the recorded
//! reason and the blocked operation index — with NO diagnostic resume
//! invocation beforehand, and the identical apply resubmission is refused
//! typed `schema_migration_in_doubt`, replaying nothing (no completion, no
//! later effect; both physical table identities stay exactly as the crash
//! left them). The tail is sown deterministically through the step-2a
//! store seam (`journal_prefix`), not by timing a real crash, and the
//! reopen uses the bounded lock-only retry (`reopen_owner_db`).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::types::schema_migration::SchemaMigrationBody;

use super::super::helpers::on_big_stack;
use super::helpers::{
    apply_request, assert_code, completions_for, empty_predecessor, field, identity, journal_kinds,
    journal_prefix, persistent_owner_db_fjall_repo, reopen_owner_db, run_apply, run_status,
    str_field, submission,
};
use crate::shamir_db::system_store::JournalEventKind;

/// The exact reconcile evidence a rename whose source AND destination are
/// present records (`apply_operations.rs`): the doubt closure prefixes
/// `operation {index} is unresolved after restart:`.
const RENAME_BOTH_PRESENT_EVIDENCE: &str = "operation 0 is unresolved after restart: rename \
     table 'main/items_1' → 'renamed_items': source and destination are in an ambiguous \
     state after restart";

async fn create_table(db: &crate::ShamirDb, name: &str) {
    let mut b = Batch::new();
    b.op("ct", ddl::create_table(name).build());
    let request = b.to_request_via_msgpack().unwrap();
    db.execute_as(super::helpers::OWNER, "testdb", &request)
        .await
        .unwrap();
}

/// A single rename of an externally pre-created table (the chunk-1
/// rename-both-present crash pattern).
fn rename_body() -> SchemaMigrationBody {
    let rename = ddl::rename_table("items_1", "renamed_items").build();
    shamir_query_builder::SchemaMigration::new("alpha-001", 1, "rename migration")
        .op(rename)
        .expect("typed rename-table operation is admitted")
        .build()
        .expect("test migration body is valid")
}

#[test]
fn restart_with_a_durable_in_doubt_tail_reports_reason_and_index_and_refuses_replay() {
    on_big_stack(|| async {
        // The FILE-BACKED repo fixture: the test asserts the physical
        // tables' identities across the restart, so the repo must survive
        // the reopen (an in-memory repo would be re-created empty).
        let (dir, db) = persistent_owner_db_fjall_repo().await;
        // The REAL ambiguous state up front: source AND destination both
        // exist, so the intent's outcome is genuinely unprovable — the
        // synthetic marker matches the physical facts instead of
        // contradicting them, and no effect is executed after it.
        create_table(&db, "items_1").await;
        create_table(&db, "renamed_items").await;
        let migration = rename_body();

        // Sow the durable tail exactly as an interrupted apply leaves it:
        // the fence (generation 1), the operation-0 intent, and the
        // best-effort InDoubt marker naming the blocked operation — then
        // the process "dies" BEFORE any DDL runs.
        journal_prefix(
            &db,
            &migration,
            &[
                (JournalEventKind::OperationIntent, 0, None),
                (
                    JournalEventKind::InDoubt,
                    0,
                    Some(RENAME_BOTH_PRESENT_EVIDENCE),
                ),
            ],
        )
        .await;
        drop(db);

        // FIRST boot after the crash (bounded lock-only retry inside
        // `reopen_owner_db`): STATUS alone carries the evidence — no
        // diagnostic resume/apply runs before this assertion.
        let db = reopen_owner_db(&dir).await;
        let target = identity(&db).await;
        let value = run_status(&db, &target).await;
        let in_flight = field(&value, "in_flight");
        assert_eq!(str_field(in_flight, "migration_id"), "alpha-001");
        assert_eq!(field(in_flight, "ordinal").as_i64(), Some(1));
        assert_eq!(field(in_flight, "fence_generation").as_i64(), Some(1));
        assert_eq!(str_field(in_flight, "status"), "in_doubt");
        assert_eq!(
            str_field(in_flight, "in_doubt_reason"),
            RENAME_BOTH_PRESENT_EVIDENCE,
            "status surfaces the recorded in-doubt reason verbatim"
        );
        assert_eq!(
            field(in_flight, "operation_index").as_i64(),
            Some(0),
            "status names the blocked operation index"
        );
        assert_eq!(str_field(&value, "error"), "schema_migration_in_doubt");

        // The identical apply resubmission — with the fence generation
        // STATUS itself reported — is refused typed in-doubt and replays
        // NOTHING: no completion, no later effect, ever.
        let error = run_apply(
            &db,
            apply_request(&target, empty_predecessor(), 1, submission(&migration)),
        )
        .await
        .expect_err("an in-doubt tail must refuse apply, never replay");
        assert_code(error, "schema_migration_in_doubt");

        let history = db
            .system_store()
            .read_schema_migration_history(target)
            .await
            .unwrap();
        assert!(history.applied.is_empty(), "nothing was replayed");
        assert_eq!(
            completions_for(&history, 0),
            0,
            "the uncertain rename was never re-run"
        );
        let kinds = journal_kinds(&db).await;
        assert!(
            !kinds.contains(&JournalEventKind::OperationComplete)
                && !kinds.contains(&JournalEventKind::Applied),
            "no completion or applied event may exist after the refusal: {kinds:?}"
        );

        // The physical state is untouched by the refusal: both table
        // identities are still exactly as the crash left them.
        let instance = db.get_db("testdb").unwrap();
        assert!(instance.has_table("main", "items_1"));
        assert!(instance.has_table("main", "renamed_items"));
    });
}
