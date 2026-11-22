//! Server-side test for the DURABILITY contract of application-schema APPLY
//! (R1c step 2b-3a): a journal write failure injected through the step-2a
//! fault seam must never let apply report success.

use std::time::Duration;

use super::super::helpers::body;
use super::helpers::{
    apply_request, assert_code, empty_predecessor, identity, persistent_owner_db, run_apply,
    submission,
};
use shamir_test_diag::FlakeGuard;

use crate::shamir_db::execute::admin_db_schema_migration::{
    AppliedRetryPause, APPLIED_RETRY_PAUSE,
};
use crate::shamir_db::system_store::JournalEventKind;

#[tokio::test]
async fn an_injected_journal_write_failure_never_returns_applied() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = body("alpha-001", 1);
    let request = apply_request(&target, empty_predecessor(), 0, submission(&migration));

    // On this fixture, tick 1 is the identity flush and tick 2 is the
    // acquisition preflush. Neither stages a journal row.
    db.system_store().arm_durability_tail_faults_after(1, 1);

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
    assert!(history.events.is_empty(), "{history:?}");
    assert!(!db.schema_migration_in_doubt("testdb"));
    assert!(!db.get_db("testdb").unwrap().has_table("main", "items_1"));
    assert!(db.schema_migration_gate_generation("testdb").is_none());
}

#[test]
fn fresh_acquire_tail_failure_keeps_the_fence_in_doubt() {
    super::super::helpers::on_big_stack(|| async {
        let scenario = Box::pin(async {
            let (_dir, db) = persistent_owner_db().await;
            let target = identity(&db).await;
            let migration = body("alpha-001", 1);
            let request = |generation| {
                apply_request(
                    &target,
                    empty_predecessor(),
                    generation,
                    submission(&migration),
                )
            };

            let error = run_apply(&db, request(1)).await.unwrap_err();
            assert!(error.to_string().contains("fence CAS conflict"), "{error}");
            assert_code(error, "schema_migration_in_progress");
            assert!(!db.schema_migration_in_doubt("testdb"));

            // Use the generic APPLY fixture's tick count below, not the
            // resume fixtures' intent-fault assumption. ACL paths differ by
            // actor; pin the actual seam with the error, history and marker.
            db.system_store().arm_durability_tail_faults_after(2, 1);
            let error = run_apply(&db, request(0)).await.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("injected durability-tail failure"),
                "{error}"
            );
            assert_code(error, "schema_migration_durability_unavailable");
            let history = db
                .system_store()
                .read_schema_migration_history(target)
                .await
                .unwrap();
            assert_eq!(history.events.len(), 1);
            assert_eq!(history.events[0].kind, JournalEventKind::Started);
            assert_eq!(history.events[0].fence_generation, 1);
            assert!(!db.get_db("testdb").unwrap().has_table("main", "items_1"));
            assert_eq!(db.schema_migration_gate_generation("testdb"), None);
            assert_eq!(
                db.schema_migration_gate_in_doubt_generation("testdb"),
                Some(1)
            );
        });
        tokio::time::timeout(Duration::from_secs(180), scenario)
            .await
            .expect("fresh acquire fault scenario timed out");
    });
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

/// Finding 1, table-driven over the two ack-relevant tails (the `Applied`
/// append and the `FenceReleased` append of the single-create-table
/// fixture; fault-tail trace as in
/// `a_post_effect_completion_sync_failure_is_not_an_applied_ack`: each
/// journal append spends its pre-transaction flush at tick N and its
/// POST-commit durable flush at N+1 — ticks 9/10 for `Applied`,
/// 11/12 for `FenceReleased`). Deterministic phases: park the original on
/// the pre-publication flush; park the ACTUAL retry inside apply after its
/// identity read and before its history read (task-local rendezvous);
/// re-park the original on the post-commit flush so the tail event is
/// visible while nothing synced it; then the retry must refuse with the
/// stable in-progress code (without the fix it returned `applied` on
/// visibility alone). The journal has a default buffered WAL, so a
/// published-but-unsynced row is the real window.
#[test]
fn an_identical_retry_never_acks_an_applied_event_before_it_is_durable() {
    super::super::helpers::on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(180),
            Box::pin(retry_races_the_tail_park(
                9,
                JournalEventKind::Applied,
                "an_identical_retry_never_acks_an_applied_event_before_it_is_durable",
            )),
        )
        .await
        .expect("applied-tail retry rendezvous scenario timed out");
    });
}

#[test]
fn an_identical_retry_never_acks_a_visible_release_before_it_is_durable() {
    super::super::helpers::on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(180),
            Box::pin(retry_races_the_tail_park(
                11,
                JournalEventKind::FenceReleased,
                "an_identical_retry_never_acks_a_visible_release_before_it_is_durable",
            )),
        )
        .await
        .expect("released-tail retry rendezvous scenario timed out");
    });
}

async fn retry_races_the_tail_park(
    preflush_tick: usize,
    expected_tail: JournalEventKind,
    scenario: &'static str,
) {
    let guard = FlakeGuard::with_watchdog(scenario, Duration::from_secs(150));
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = body("alpha-001", 1);
    let request = apply_request(&target, empty_predecessor(), 0, submission(&migration));

    // Phase 1: park the original BEFORE it publishes the tail event.
    let pre_hook = db
        .system_store()
        .arm_durability_tail_rendezvous(preflush_tick);
    let runner = {
        let db = db.clone();
        let request = request.clone();
        tokio::spawn(async move { run_apply(&db, request).await })
    };
    pre_hook.parked().await;
    guard.mark("original parked pre-publication");
    assert_eq!(
        db.schema_migration_gate_generation("testdb"),
        Some(1),
        "the original runner holds the live fence before it publishes"
    );

    // Phase 2: start the actual retry and park it inside apply, after its
    // identity read and before its history read.
    let pause = AppliedRetryPause::armed();
    let retry = APPLIED_RETRY_PAUSE.scope(pause.clone(), {
        let db = db.clone();
        let request = request.clone();
        async move { run_apply(&db, request).await }
    });
    let retry = tokio::spawn(retry);
    pause.reached().await;
    guard.mark("retry parked post-identity");

    // Phase 3: publish (the original's next tail) and re-park on the
    // post-commit durable flush. No other tail runs in between.
    let post_hook = db.system_store().arm_durability_tail_rendezvous(1);
    pre_hook.release();
    post_hook.parked().await;
    guard.mark("original parked post-publication");
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(
        history.events.last().map(|event| event.kind),
        Some(expected_tail),
        "the parked window is exactly publish-before-durability: {history:?}"
    );
    assert_eq!(db.schema_migration_gate_generation("testdb"), Some(1));

    // Phase 4: the retry must refuse, not acknowledge.
    pause.release();
    let error = retry
        .await
        .expect("retry task joins")
        .expect_err("a retry must not acknowledge before the observed event is durable");
    assert_code(error, "schema_migration_in_progress");
    guard.mark("retry refused while the tail is parked");

    // Phase 5: the original finishes its durable tail and release; only
    // then does a retry acknowledge as a no-op.
    post_hook.release();
    runner
        .await
        .expect("runner task joins")
        .expect("the original runner completes once its tail is released");
    run_apply(&db, request)
        .await
        .expect("a retry after the durable release acknowledges as a no-op");
    guard.mark("post-release retry acknowledged");
    assert_eq!(db.schema_migration_gate_generation("testdb"), None);
    assert!(!db.schema_migration_in_doubt("testdb"));
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
            JournalEventKind::Applied,
            JournalEventKind::FenceReleased,
        ],
        "{history:?}"
    );
}

/// After a FAILED release tail the marker is retained while the release row
/// is merely visible (ticks 11–12: arming after 11 fails exactly the
/// post-commit durable flush of the release append; its failure surfaces
/// through `durability_error(.., true)` as the stable in-doubt code). The
/// retry must not acknowledge on visibility alone and must not append a
/// duplicate release: the fix routes it through the journal partition's
/// explicit durable tail, the same `flush_system_repo_durable` seam as
/// every other tail. Arming `arm_durability_tail_faults_after(1, 1)` lets
/// the retry's first tail (its identity read) pass and faults the SECOND —
/// the explicit journal tail — so the retry refuses with the stable
/// in-doubt code and keeps the marker; the next retry, with the tail
/// available, acknowledges while the marker stays for reopen
/// reconciliation.
#[test]
fn a_retry_after_a_failed_release_tail_syncs_before_it_acks() {
    super::super::helpers::on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(180),
            Box::pin(retry_after_failed_release_tail()),
        )
        .await
        .expect("failed-release-tail retry scenario timed out");
    });
}

async fn retry_after_failed_release_tail() {
    let guard = FlakeGuard::with_watchdog(
        "a_retry_after_a_failed_release_tail_syncs_before_it_acks",
        Duration::from_secs(150),
    );
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = body("alpha-001", 1);
    let request = apply_request(&target, empty_predecessor(), 0, submission(&migration));

    db.system_store().arm_durability_tail_faults_after(11, 1);
    let error = run_apply(&db, request.clone())
        .await
        .expect_err("the failed release tail must not report success");
    assert_code(error, "schema_migration_in_doubt");
    guard.mark("original failed its release tail");
    assert_eq!(db.schema_migration_gate_generation("testdb"), None);
    assert_eq!(
        db.schema_migration_gate_in_doubt_generation("testdb"),
        Some(1),
        "the failed run retains its in-doubt marker"
    );
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(
        history.events.last().map(|event| event.kind),
        Some(JournalEventKind::FenceReleased),
        "the release row is visible but its tail failed: {history:?}"
    );

    // The retry's next tail is its explicit pre-acknowledgement sync; fail it.
    db.system_store().arm_durability_tail_faults_after(1, 1);
    let error = run_apply(&db, request.clone())
        .await
        .expect_err("a retry must not acknowledge while its sync fails");
    assert_code(error, "schema_migration_in_doubt");
    assert_eq!(
        db.schema_migration_gate_in_doubt_generation("testdb"),
        Some(1),
        "the retained marker survives the refused retry"
    );
    guard.mark("retry refused while its explicit tail fails");

    // With the tail available the retry acknowledges; the marker is never
    // cleared on visibility alone and no duplicate release was appended.
    run_apply(&db, request)
        .await
        .expect("the retry acknowledges once its sync succeeds");
    guard.mark("retry acknowledged after its explicit tail");
    assert_eq!(
        db.schema_migration_gate_in_doubt_generation("testdb"),
        Some(1),
        "the retained marker stays for reopen reconciliation"
    );
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    let releases = history
        .events
        .iter()
        .filter(|event| event.kind == JournalEventKind::FenceReleased)
        .count();
    assert_eq!(
        releases, 1,
        "no duplicate release was appended: {history:?}"
    );
}
