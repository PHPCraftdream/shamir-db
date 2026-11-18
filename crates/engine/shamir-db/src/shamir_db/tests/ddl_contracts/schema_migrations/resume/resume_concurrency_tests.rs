//! Server-side tests for CONCURRENT resumes and FOLLOWER refusal of an
//! interrupted application-schema APPLY (R1c step 2b-3b). Crashes are the
//! chunk-1 fault-seam pattern (`arm_durability_tail_faults_after(2, 1)`
//! leaves the journal with `started` only).

use super::helpers::{
    apply_request, assert_code, crash_started_only_and_reopen, empty_predecessor, identity,
    journal_kinds, run_apply, str_field, submission,
};
use crate::shamir_db::system_store::JournalEventKind;

fn kind_count(kinds: &[JournalEventKind], kind: JournalEventKind) -> usize {
    kinds.iter().filter(|event| **event == kind).count()
}

#[tokio::test]
async fn two_concurrent_resumes_exactly_one_runs() {
    let (_dir, db, migration) = crash_started_only_and_reopen().await;
    let target = identity(&db).await;
    let request = || apply_request(&target, empty_predecessor(), 0, submission(&migration));

    // Determinism of "exactly one": admission step 6 checks the in-process
    // gate BEFORE the durable fence CAS of step 10, and the winner holds that
    // gate from its acquire until its release — so a loser that passes step 6
    // before the gate is installed is STILL fenced by the step-10 durable CAS
    // (a single-writer fence; its refusal maps to `in_progress`). No
    // interleaving can make both runners write.
    let (winner, loser) = tokio::join!(run_apply(&db, request()), run_apply(&db, request()));

    // The loser is either refused in progress (gate held at admission step 6,
    // or the step-10 durable CAS conflict) or answered as the durable no-op —
    // both are documented non-double-run outcomes.
    match loser {
        Ok(value) => assert_eq!(str_field(&value, "status"), "applied"),
        Err(error) => assert_code(error, "schema_migration_in_progress"),
    }

    let value = winner.expect("exactly one resume run must succeed");
    assert_eq!(str_field(&value, "status"), "applied");

    // Journal proof of a single run: exactly one Started (the crashed run),
    // one RecoveryAcquired (the single takeover), one intent, one completion,
    // and one Applied — a second writing runner would duplicate these.
    let kinds = journal_kinds(&db).await;
    assert_eq!(kind_count(&kinds, JournalEventKind::Started), 1);
    assert_eq!(kind_count(&kinds, JournalEventKind::RecoveryAcquired), 1);
    assert_eq!(kind_count(&kinds, JournalEventKind::OperationIntent), 1);
    assert_eq!(kind_count(&kinds, JournalEventKind::OperationComplete), 1);
    assert_eq!(kind_count(&kinds, JournalEventKind::Applied), 1);
}

#[tokio::test]
async fn follower_refuses_resume() {
    let (_dir, db, migration) = crash_started_only_and_reopen().await;
    let target = identity(&db).await;

    // The engine-side primary-only check (apply admission step 2, fail-closed
    // behind the server's own follower gate) runs BEFORE any journal read or
    // write, so the follower refusal cannot resume or mutate the journal.
    db.set_node_read_only(true);
    let error = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect_err("a follower must not resume an unresolved migration");
    assert_code(error, "schema_migration_follower_read_only");
    assert_eq!(
        journal_kinds(&db).await,
        vec![JournalEventKind::Started],
        "the follower refusal writes nothing"
    );

    // Lowering the flag restores the primary path: the identical submission
    // resumes the unresolved journal normally.
    db.set_node_read_only(false);
    let value = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("the identical submission resumes once the node is primary");
    assert_eq!(str_field(&value, "status"), "applied");
    assert_eq!(field_fence_generation(&value), 2);
}

fn field_fence_generation(value: &crate::types::value::QueryValue) -> i64 {
    super::helpers::field(value, "fence_generation")
        .as_i64()
        .expect("fence_generation is an integer")
}
