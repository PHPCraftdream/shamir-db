//! Server-side test for the DURABILITY contract of application-schema APPLY
//! (R1c step 2b-3a): a journal write failure injected through the step-2a
//! fault seam must never let apply report success.

use std::time::Duration;

use super::super::helpers::body;
use super::helpers::{
    apply_request, assert_code, empty_predecessor, identity, persistent_owner_db, run_apply,
    submission,
};
use crate::shamir_db::system_store::JournalEventKind;

#[tokio::test]
async fn an_injected_journal_write_failure_never_returns_applied() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = body("alpha-001", 1);
    let request = apply_request(&target, empty_predecessor(), 0, submission(&migration));

    // Arm the seam so the FIRST durable catalogue tail of the apply run —
    // the fence acquisition's preflight flush — fails (see
    // `arm_durability_tail_faults_after` in the fence tests: the acquisition
    // spends its first tail on the preflight).
    db.system_store().arm_durability_tail_faults(1);

    let error = run_apply(&db, request)
        .await
        .expect_err("a failed journal write must never report success");
    assert_code(error, "schema_migration_durability_unavailable");

    // Nothing was applied: no applied event, no completion, no object.
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert!(history.applied.is_empty(), "{history:?}");
    assert!(
        !history.events.iter().any(|event| matches!(
            event.kind,
            JournalEventKind::Applied | JournalEventKind::OperationComplete
        )),
        "no completion may survive the failure: {history:?}"
    );
    assert!(!db.get_db("testdb").unwrap().has_table("main", "items_1"));
    assert!(db.schema_migration_gate_generation("testdb").is_none());
}

/// A TRUE post-effect journal sync failure on the STANDARD fixture — no
/// reopen, no replay: the whole scenario runs against the live engine.
/// Observed fault-tail trace of one single-create-table apply (every durable
/// tail spends one fault-gate tick through `flush_system_repo_durable` /
/// `flush_journal_tail`; each journal append spends two — the
/// pre-transaction flush and the post-commit durable flush of
/// `commit_transition`): the pre-acquisition identity read = tick 1, fence
/// acquisition = 2–3, operation-0 intent
/// append = 4–5, the CreateTable effect's table-catalogue write
/// (`table_management.rs` `SystemStore::save_table` →
/// `catalogue/ops.rs::flush_system_repo_durable`) = tick 6, the completion
/// append's preflush = 7, its POST-commit durable flush = 8. Arming
/// `after(7, 1)` therefore fails exactly the completion append's post-commit
/// durable flush: after the DDL effect ran and after the completion row
/// committed to the journal table, before any of it is durable. With the
/// intent already durable, `durability_error(.., true)` reports
/// `schema_migration_in_doubt`, never a plain unavailability code.
#[test]
fn a_post_effect_completion_sync_failure_is_not_an_applied_ack() {
    super::super::helpers::on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(180),
            Box::pin(post_effect_sync_failure()),
        )
        .await
        .expect("post-effect completion sync failure scenario timed out");
    });
}

async fn post_effect_sync_failure() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = body("alpha-001", 1);
    let request = apply_request(&target, empty_predecessor(), 0, submission(&migration));

    db.system_store().arm_durability_tail_faults_after(7, 1);

    let error = run_apply(&db, request)
        .await
        .expect_err("a failed post-effect completion sync must never report success");
    assert_code(error.clone(), "schema_migration_in_doubt");
    assert!(
        error
            .to_string()
            .contains("injected durability-tail failure"),
        "the injected storage fault must stay visible in the refusal: {error}"
    );

    // The effect exists: the failure is AFTER the DDL ran (the migration's
    // next effect — none, it is a single-operation body — can never have
    // run).
    assert!(
        db.get_db("testdb").unwrap().has_table("main", "items_1"),
        "the DDL effect ran before the failed sync"
    );

    // The journal shows exactly the pre-effect events plus the completion
    // row whose durable flush failed; no applied ack survived.
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    let kinds: Vec<JournalEventKind> = history.events.iter().map(|event| event.kind).collect();
    assert_eq!(
        kinds,
        vec![
            JournalEventKind::Started,
            JournalEventKind::OperationIntent,
            JournalEventKind::OperationComplete,
        ],
        "the failure is exactly at the completion's post-commit flush: {history:?}"
    );
    let completion = history
        .events
        .iter()
        .find(|event| event.kind == JournalEventKind::OperationComplete)
        .unwrap();
    assert_eq!(completion.operation_index, Some(0));
    assert!(
        history.applied.is_empty(),
        "no applied ack may survive the failed sync: {history:?}"
    );

    // The LIVE gate is gone (the `LiveGate` Drop converted the failed run)
    // and the in-doubt marker stands with the interrupted run's known
    // generation.
    assert!(
        db.schema_migration_gate_generation("testdb").is_none(),
        "a failed run must not keep a live gate"
    );
    assert_eq!(
        db.schema_migration_gate_in_doubt_generation("testdb"),
        Some(1),
        "the run stays in doubt at its acquisition generation"
    );
}
