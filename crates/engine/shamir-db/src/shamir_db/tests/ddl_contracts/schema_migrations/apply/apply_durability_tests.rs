//! Server-side test for the DURABILITY contract of application-schema APPLY
//! (R1c step 2b-3a): a journal write failure injected through the step-2a
//! fault seam must never let apply report success.

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
