//! Journal isolation (R1c step 4, design §5.3): a deliberately corrupted /
//! unadmitted journal event of database A fails A closed with its own typed
//! error on status, plan and apply, while database B's status, plan and a
//! healthy apply keep working on the same journal table. The RESTART variant
//! sows the same body damage through the WAL-backed implicit-transaction
//! seam (`helpers.rs`) and expects the WHOLE OPEN after the cold restart to
//! fail closed with the typed `DbError::SchemaMigrationJournalUnreadable`.
//!
//! Requests are built ONLY through `shamir-query-builder` and executed
//! through the normal batch entry point (`execute` / `execute_as`).

use std::time::Duration;

use shamir_query_builder::ddl;
use shamir_query_types::admin::types::schema_migration::SchemaMigrationOp;

use super::super::helpers::on_big_stack;
use super::super::helpers::{
    body, checksum, context, fence_generation_from_status, field, journal_table, overwrite_row,
    persistent_db, predecessor, prepare_database, rows, run_apply, run_plan, run_status, str_field,
    submission, try_init_waiting_for_locks, wal_patch_journal_row,
};
use crate::query::batch::BatchError;
use crate::shamir_db::system_store::JournalEvent;
use crate::types::value::QueryValue;
use crate::ShamirDb;

/// Append one REAL extra allowed operation to a stored `JournalEvent`
/// payload: the record stays well-formed and admitted — only the checksum no
/// longer matches its stored body.
fn append_extra_operation(payload: &[u8]) -> Vec<u8> {
    let mut event: JournalEvent = rmp_serde::from_slice(payload).unwrap();
    let body = event
        .body
        .as_mut()
        .expect("started event stores its migration body");
    // A REAL extra allowed operation is appended, so the record is well-formed
    // and admitted — only the checksum no longer matches its stored body.
    let extra = ddl::create_table("unplanned_items").build();
    body.operations
        .push(SchemaMigrationOp::try_from(extra).expect("typed operation is admitted"));
    rmp_serde::to_vec_named(&event).unwrap()
}

/// Corrupt ONLY the stored body of the first journal event: the reader must
/// refuse it as an integrity failure for its own database. Same-process
/// variant: the raw no-WAL seam (`overwrite_row`) is enough — the engine is
/// never dropped, so nothing has to survive recovery.
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
    if let QueryValue::Map(fields) = &mut envelope {
        fields.insert(
            "event".into(),
            QueryValue::Bin(append_extra_operation(&payload)),
        );
    }
    overwrite_row(&table, event_id, envelope).await;
}

/// RESTART variant: the same body damage, but committed through the
/// WAL-BACKED implicit-transaction seam (builder-only typed update on the
/// unique `record_key`) — see `helpers.rs`. The no-WAL raw seam above would
/// be lost on reopen: recovery replays the last committed WAL state and
/// rematerializes the CLEAN body, which is exactly what the pre-fix restart
/// fixture observed.
async fn corrupt_first_event_body_via_wal(db: &ShamirDb) {
    let table = journal_table(db).await;
    let envelope = rows(&table)
        .await
        .into_iter()
        .find(|(_, row)| row.get("record_type").and_then(QueryValue::as_str) == Some("event"))
        .expect("the started event row is persisted")
        .1;
    drop(table);
    let record_key = envelope
        .get("record_key")
        .and_then(QueryValue::as_str)
        .expect("the started event row carries its record_key")
        .to_string();
    let payload = match envelope.get("event") {
        Some(QueryValue::Bin(bytes)) => bytes.clone(),
        _ => panic!("started event payload is binary"),
    };
    let mutated = append_extra_operation(&payload);
    let mut fields = shamir_types::types::common::new_map();
    fields.insert("event".to_string(), QueryValue::Bin(mutated));
    wal_patch_journal_row(db, &record_key, QueryValue::Map(fields)).await;
}

fn assert_closed(error: BatchError, context: &str) {
    assert_eq!(
        error.code(),
        Some("schema_migration_durability_unavailable"),
        "{context}: wrong refusal code; message: {error}"
    );
    // The corruption cause carries through in the error message.
    assert!(
        error
            .to_string()
            .contains("body metadata/checksum mismatch"),
        "{context}: expected the journal corruption cause, got: {error}"
    );
}

/// The DbError-side twin of [`assert_closed`]: the EXACT
/// `DbError::SchemaMigrationJournalUnreadable` variant (not just any
/// `durability_unavailable`-coded error) and its stable `.code()`.
fn assert_unreadable(error: &crate::DbError, scenario: &str) {
    assert!(
        matches!(error, crate::DbError::SchemaMigrationJournalUnreadable(_)),
        "{scenario}: wrong startup failure variant; message: {error}"
    );
    assert_eq!(
        error.code(),
        Some("schema_migration_durability_unavailable"),
        "{scenario}: wrong startup failure code; message: {error}"
    );
}

#[test]
fn corrupted_event_of_one_database_fails_it_closed_and_never_blocks_another_database() {
    // Debug-test stacks are small: run the scenario on a roomy thread, bounded
    // by a generous hang-detector timeout (the Windows runner is up to 10x
    // slower).
    on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(300),
            Box::pin(corrupted_event_scenario()),
        )
        .await
        .expect("journal isolation scenario timed out");
    });
}

#[test]
fn corrupted_event_of_one_database_fails_it_closed_after_a_cold_restart() {
    // Same body damage, but committed through the WAL-backed seam and
    // flushed durably before the engine is dropped: the cold restart must
    // FAIL THE WHOLE OPEN closed with the typed journal-unreadable error.
    on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(300),
            Box::pin(corrupted_event_whole_open_scenario()),
        )
        .await
        .expect("journal isolation restart scenario timed out");
    });
}

async fn corrupted_event_scenario() {
    let (_dir, db) = persistent_db().await;
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
    assert_closed(status_error, "status");
    let plan_error = run_plan(
        &db,
        "alpha",
        predecessor(None, 0, [0; 32]),
        vec![submission(&body("alpha-001", 1))],
        &alpha,
    )
    .await
    .expect_err("database A plan must fail closed on its corrupted journal");
    assert_closed(plan_error, "plan");
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
    assert_closed(apply_error, "apply");

    // B still reads healthy after A's failures: nothing in A's journal
    // poisoned B's view.
    let status_after = run_status(&db, "beta", &beta)
        .await
        .expect("database B status stays healthy after A's refusals");
    assert_eq!(str_field(&status_after, "last_applied_id"), "beta-001");
}

async fn corrupted_event_whole_open_scenario() {
    let (dir, db) = persistent_db().await;
    db.create_db("alpha").await.unwrap();
    prepare_database(&db, "alpha").await;
    let alpha = db
        .system_store()
        .schema_migration_identity("alpha")
        .await
        .unwrap();

    // The same Started-event body damage as the same-process scenario.
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

    // Sow the damage through the WAL-BACKED seam, flush durably, drop the
    // engine: only the WAL-backed damage survives a cold restart.
    corrupt_first_event_body_via_wal(&db).await;
    let table = journal_table(&db).await;
    let system_path = dir.path().join("system");
    db.system_store()
        .flush_system_repo_durable(&table)
        .await
        .expect("the damaged journal row flushes durably before the restart");
    drop(table);
    drop(db);

    // Whole-open on body damage: the cold restart must FAIL CLOSED with the
    // typed journal-unreadable error — never a partial open, and therefore
    // never any post-open action to assert on.
    let Err(error) = try_init_waiting_for_locks(system_path).await else {
        panic!("restart must fail closed on the corrupted journal row");
    };
    assert_unreadable(&error, "restart on a corrupted journal body");
    // Keep the tempdir alive to the end of the scenario.
    drop(dir);
}
