//! Journal isolation (R1c step 4, design §5.3): a deliberately corrupted /
//! unadmitted journal event of database A fails A closed with its own typed
//! error on status, plan and apply, while database B's status, plan and a
//! healthy apply keep working on the same journal table.
//!
//! Requests are built ONLY through `shamir-query-builder` and executed
//! through the normal batch entry point (`execute` / `execute_as`).

use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::types::schema_migration::SchemaMigrationOp;

use super::super::helpers::on_big_stack;
use super::super::helpers::{
    body, checksum, context, fence_generation_from_status, field, flush_and_reopen, journal_table,
    overwrite_row, persistent_db, predecessor, prepare_database, rows, run_apply, run_plan,
    run_status, str_field, submission, OWNER,
};
use crate::query::batch::BatchError;
use crate::shamir_db::system_store::JournalEvent;
use crate::types::value::QueryValue;
use crate::ShamirDb;

/// Corrupt ONLY the stored body of the first journal event: the reader must
/// refuse it as an integrity failure for its own database.
async fn corrupt_first_event_body(db: &ShamirDb) {
    let table = journal_table(db).await;
    let (event_id, mut envelope) = rows(&table)
        .await
        .into_iter()
        .find(|(_, row)| row.get("record_type").and_then(QueryValue::as_str) == Some("event"))
        .unwrap();
    let payload = match envelope.get("event") {
        Some(QueryValue::Bin(bytes)) => bytes.clone(),
        _ => panic!("started event payload is binary"),
    };
    let mut event: JournalEvent = rmp_serde::from_slice(&payload).unwrap();
    let body = event
        .body
        .as_mut()
        .expect("started event stores its migration body");
    // A REAL extra allowed operation is appended, so the record is well-formed
    // and admitted — only the checksum no longer matches its stored body.
    let extra = ddl::create_table("unplanned_items").build();
    body.operations
        .push(SchemaMigrationOp::try_from(extra).expect("typed operation is admitted"));
    if let QueryValue::Map(fields) = &mut envelope {
        fields.insert(
            "event".into(),
            QueryValue::Bin(rmp_serde::to_vec_named(&event).unwrap()),
        );
    }
    overwrite_row(&table, event_id, envelope).await;
}

fn assert_closed(error: BatchError, context: &str, detailed: bool) {
    assert_eq!(
        error.code(),
        Some("schema_migration_durability_unavailable"),
        "{context}: wrong refusal code; message: {error}"
    );
    if detailed {
        // Same-process: the corruption cause carries through. After a restart
        // the open-time in-doubt marker reports the generic refusal, so the
        // detailed message is asserted only for the no-reopen variant.
        assert!(
            error
                .to_string()
                .contains("body metadata/checksum mismatch"),
            "{context}: expected the journal corruption cause, got: {error}"
        );
    }
}

/// A plain DDL writer batch against the damaged database, pinned to the
/// database's `main` repository: after a restart the open-time in-doubt
/// marker must refuse it with the TYPED in-doubt code, never a silent
/// success (and never an actually executed create-table).
fn writer_batch() -> Batch {
    let mut b = Batch::new();
    b.op(
        "ct",
        ddl::create_table("blocked_items").repo("main").build(),
    );
    b
}

async fn run_writer(db: &ShamirDb, database: &str) -> Result<(), BatchError> {
    let request = writer_batch().to_request_via_msgpack().unwrap();
    db.execute_as(OWNER, database, &request).await?;
    Ok(())
}

#[test]
fn corrupted_event_of_one_database_fails_it_closed_and_never_blocks_another_database() {
    // Debug-test stacks are small: run the scenario on a roomy thread, bounded
    // by a generous hang-detector timeout (the Windows runner is up to 10x
    // slower).
    on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(300),
            Box::pin(corrupted_event_scenario(false)),
        )
        .await
        .expect("journal isolation scenario timed out");
    });
}

#[test]
fn corrupted_event_of_one_database_fails_it_closed_after_a_cold_restart() {
    // Same scenario, but the damaged journal is flushed durably and the
    // system store is REOPENED before the assertions: the corruption must
    // fail database A closed across a restart too, while B stays healthy.
    on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(300),
            Box::pin(corrupted_event_scenario(true)),
        )
        .await
        .expect("journal isolation restart scenario timed out");
    });
}

async fn corrupted_event_scenario(reopen: bool) {
    let (dir, db) = persistent_db().await;
    db.create_db("alpha").await.unwrap();
    db.create_db("beta").await.unwrap();
    prepare_database(&db, "alpha").await;
    prepare_database(&db, "beta").await;
    let alpha = db
        .system_store()
        .schema_migration_identity("alpha")
        .await
        .unwrap();
    let beta = db
        .system_store()
        .schema_migration_identity("beta")
        .await
        .unwrap();

    // Damage A first: one Started event whose stored body no longer matches
    // its recorded checksum (unadmitted under the current rules).
    let damaged = body("alpha-001", 1);
    db.system_store()
        .acquire_schema_migration_fence(
            alpha,
            0,
            damaged.metadata.clone(),
            damaged,
            false,
            &context(),
        )
        .await
        .unwrap();
    corrupt_first_event_body(&db).await;
    let db = if reopen {
        // Persist the damaged journal row durably, then drop the engine and
        // reopen: the corruption must survive a cold restart of the store.
        let table = journal_table(&db).await;
        flush_and_reopen(dir.path(), db, table).await
    } else {
        db
    };

    // B is untouched by A's damaged record: its apply succeeds. The apply
    // generation comes from B's OWN status, never a magic constant.
    let beta_first = body("beta-001", 1);
    let applied = run_apply(
        &db,
        "beta",
        predecessor(None, 0, [0; 32]),
        fence_generation_from_status(&db, "beta", &beta).await,
        submission(&beta_first),
        &beta,
    )
    .await
    .expect("database B applies its first migration beside a corrupted A journal");
    assert_eq!(str_field(&applied, "status"), "applied");
    assert_eq!(str_field(&applied, "migration_id"), "beta-001");

    // B's status reports the applied prefix and its plan of the next ordinal
    // still works.
    let status = run_status(&db, "beta", &beta)
        .await
        .expect("database B status reads past the corrupted A record");
    assert_eq!(str_field(&status, "last_applied_id"), "beta-001");
    assert_eq!(field(&status, "last_applied_ordinal").as_i64(), Some(1));
    assert_eq!(str_field(&status, "checksum_validation"), "valid");
    let beta_next = body("beta-002", 2);
    let planned = run_plan(
        &db,
        "beta",
        predecessor(Some("beta-001"), 1, checksum(&beta_first)),
        vec![submission(&beta_first), submission(&beta_next)],
        &beta,
    )
    .await
    .expect("database B plans its next migration beside a corrupted A journal");
    assert_eq!(str_field(field(&planned, "migration"), "id"), "beta-002");

    // A fails closed on all three operations with its own typed corruption
    // error — never a guess, and never blocking B.
    let status_error = run_status(&db, "alpha", &alpha)
        .await
        .expect_err("database A status must fail closed on its corrupted journal");
    assert_closed(status_error, "status", !reopen);
    let plan_error = run_plan(
        &db,
        "alpha",
        predecessor(None, 0, [0; 32]),
        vec![submission(&body("alpha-001", 1))],
        &alpha,
    )
    .await
    .expect_err("database A plan must fail closed on its corrupted journal");
    assert_closed(plan_error, "plan", !reopen);
    let apply_error = run_apply(
        &db,
        "alpha",
        predecessor(None, 0, [0; 32]),
        0,
        submission(&body("alpha-001", 1)),
        &alpha,
    )
    .await
    .expect_err("database A apply must fail closed on its corrupted journal");
    assert_closed(apply_error, "apply", !reopen);

    if reopen {
        // After the restart the open-time in-doubt marker refuses a plain
        // DDL writer against A with the EXACT in-doubt code, and the
        // refused create-table was never executed.
        let writer_error = run_writer(&db, "alpha")
            .await
            .expect_err("restart: database A must refuse a DDL writer");
        assert_eq!(
            writer_error.code(),
            Some("schema_migration_in_doubt"),
            "restart: wrong writer refusal code; message: {writer_error}"
        );
        let instance = db.get_db("alpha").unwrap();
        assert!(
            !instance.has_table("main", "blocked_items"),
            "the refused writer must not have executed its create-table"
        );

        // Positive control for the healthy database is already asserted
        // above: B's apply through the normal batch succeeded after the
        // restart and its status still reports the unchanged control row
        // (beta-001, ordinal 1).
    }

    // B still reads healthy after A's failures: nothing in A's journal
    // poisoned B's view.
    let status_after = run_status(&db, "beta", &beta)
        .await
        .expect("database B status stays healthy after A's refusals");
    assert_eq!(str_field(&status_after, "last_applied_id"), "beta-001");
}
