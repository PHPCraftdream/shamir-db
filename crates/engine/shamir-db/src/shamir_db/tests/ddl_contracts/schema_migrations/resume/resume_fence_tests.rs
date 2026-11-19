//! Server-side tests for the RESTART FENCE of an interrupted
//! application-schema APPLY (R1c step 2b-3b): the open-time in-doubt marker
//! refuses writes until a resume converts it into a live gate, a takeover
//! fences the stale runner's generation, and mismatched resubmissions are
//! refused without any journal write. Crashes are simulated deterministically
//! through the step-2a durability-tail fault seam: each journal append spends
//! two fault-gate ticks (pre-transaction flush, post-commit flush), so
//! `arm_durability_tail_faults_after(2, 1)` fails the first operation-intent
//! append (append 2, tick 3) and leaves the journal with `started` only.

use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::write::{doc, insert};
use shamir_query_builder::Query;
use shamir_query_builder::SchemaMigration;
use shamir_query_types::admin::types::schema_migration::SchemaMigrationBody;
use shamir_test_diag::FlakeGuard;
use tokio::time::timeout;

use super::super::helpers::{body, checksum, context, on_big_stack};
use super::helpers::{
    apply_request, assert_code, empty_predecessor, field, identity, journal_rows,
    persistent_owner_db, predecessor, reopen_owner_db, run_apply, run_status, str_field,
    submission,
};
use crate::shamir_db::system_store::{JournalEventKind, SchemaMigrationStep};
use crate::shamir_db::ShamirDb;

async fn create_table(db: &ShamirDb, name: &str) {
    let mut b = Batch::new();
    b.op("ct", ddl::create_table(name).build());
    let request = b.to_request_via_msgpack().unwrap();
    db.execute_as(super::helpers::OWNER, "testdb", &request)
        .await
        .unwrap();
}

fn write_batch() -> Batch {
    let mut b = Batch::new();
    b.op(
        "ins",
        insert("probe").row(
            doc()
                .set("name", "fenced-out")
                .expect("literal doc field interns"),
        ),
    );
    b
}

/// Crash a one-operation create migration at its first intent append
/// (journal left with `started` only) and reopen the same directory. A
/// `probe` table is created BEFORE the crash so reads have a target after
/// the restart (creating it afterwards would itself be fenced).
async fn crash_started_only_and_reopen() -> (tempfile::TempDir, ShamirDb, SchemaMigrationBody) {
    let (dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    create_table(&db, "probe").await;
    let migration = body("alpha-001", 1);
    db.system_store().arm_durability_tail_faults_after(2, 1);
    let error = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect_err("the failed intent append must stop the run");
    assert_code(error, "schema_migration_durability_unavailable");
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(history.events.len(), 1);
    assert_eq!(history.events[0].kind, JournalEventKind::Started);
    drop(db);
    let db = reopen_owner_db(&dir).await;
    (dir, db, migration)
}

#[tokio::test]
async fn writers_refused_in_doubt_after_restart_until_resume() {
    let (_dir, db, migration) = crash_started_only_and_reopen().await;

    // The reopen seeded the in-doubt marker: writes are refused with the
    // in-doubt code...
    let error = db
        .execute_as(
            super::helpers::OWNER,
            "testdb",
            &write_batch().to_request_via_msgpack().unwrap(),
        )
        .await
        .expect_err("a write against the in-doubt database must be refused");
    assert_code(error, "schema_migration_in_doubt");

    // ...while reads stay available.
    let mut read = Batch::new();
    read.op("read", Query::from("probe"));
    db.execute_as(
        super::helpers::OWNER,
        "testdb",
        &read.to_request_via_msgpack().unwrap(),
    )
    .await
    .expect("reads stay available under the in-doubt marker");

    // The resume resolves the journal, releases the fence and clears the
    // marker.
    let target = identity(&db).await;
    let value = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("the identical submission resumes the unresolved migration");
    assert_eq!(str_field(&value, "status"), "applied");

    // The same write now passes.
    db.execute_as(
        super::helpers::OWNER,
        "testdb",
        &write_batch().to_request_via_msgpack().unwrap(),
    )
    .await
    .expect("the same write is admitted after the resume");
    assert!(db.schema_migration_gate_generation("testdb").is_none());
    assert!(db
        .schema_migration_gate_in_doubt_generation("testdb")
        .is_none());
}

#[tokio::test]
async fn the_fence_distinguishes_in_doubt_marker_live_gate_and_free() {
    let (_dir, db, _migration) = crash_started_only_and_reopen().await;

    // Marker only (no live runner after the restart): in doubt.
    assert_eq!(
        db.schema_migration_gate_in_doubt_generation("testdb"),
        Some(1),
        "the open-time seed records the crashed run's generation"
    );
    assert!(db.schema_migration_gate_generation("testdb").is_none());
    let error = db
        .execute_as(
            super::helpers::OWNER,
            "testdb",
            &write_batch().to_request_via_msgpack().unwrap(),
        )
        .await
        .expect_err("marker-only must refuse writes with in doubt");
    assert_code(error, "schema_migration_in_doubt");

    // A resumed runner installs the LIVE gate (and clears the marker): the
    // same write is now refused with in progress, deterministically — the
    // hold is exactly what a live resume does.
    db.hold_schema_migration_gate("testdb", 1);
    assert!(db
        .schema_migration_gate_in_doubt_generation("testdb")
        .is_none());
    let error = db
        .execute_as(
            super::helpers::OWNER,
            "testdb",
            &write_batch().to_request_via_msgpack().unwrap(),
        )
        .await
        .expect_err("a live gate must refuse writes with in progress");
    assert_code(error, "schema_migration_in_progress");

    // After the release (the resumed run's RAII exit) neither state is held:
    // the write is admitted.
    db.release_schema_migration_gate("testdb", 1);
    db.execute_as(
        super::helpers::OWNER,
        "testdb",
        &write_batch().to_request_via_msgpack().unwrap(),
    )
    .await
    .expect("the write is admitted once nothing is held");
}

#[test]
fn takeover_fences_the_stale_runner_generation() {
    on_big_stack(|| async {
        // Whole-scenario bound (>=120 s: the Windows runner is up to 10x
        // slower) with the failure-only diagnostic watchdog of the
        // boot-oracle pattern: the watchdog fires BELOW the runner's
        // slow-timeout kill so the timeline dumps first; the timeout itself
        // fails the test.
        let guard = FlakeGuard::with_watchdog(
            "takeover_fences_the_stale_runner_generation",
            Duration::from_secs(150),
        );
        let scenario = Box::pin(async {
            let (_dir, db, migration) = crash_started_only_and_reopen().await;
            let target = identity(&db).await;

            // Take over via resume: a FRESH fence generation.
            let value = run_apply(
                &db,
                apply_request(&target, empty_predecessor(), 0, submission(&migration)),
            )
            .await
            .expect("the resume takes over the owned fence");
            assert_eq!(str_field(&value, "status"), "applied");

            // The stale runner (old generation 1) cannot append through the system
            // store: the durable fence CAS refuses it.
            let error = db
                .system_store()
                .append_schema_migration_event(
                    target,
                    1,
                    migration.metadata.clone(),
                    checksum(&migration),
                    SchemaMigrationStep {
                        kind: JournalEventKind::OperationIntent,
                        operation_index: Some(0),
                        evidence: None,
                    },
                    &context(),
                )
                .await
                .expect_err("a stale generation must not append");
            assert!(
                error.to_string().contains("fence CAS conflict"),
                "the step-2a fence CAS refuses the stale runner: {error}"
            );

            // And a stale runner cannot execute through the in-process path either:
            // while a live gate is held (as the resumed runner holds it), the NEXT
            // migration is refused in progress. (The identical migration is already
            // applied and answers as the durable no-op before the gate.)
            db.hold_schema_migration_gate("testdb", 2);
            let next = body("alpha-002", 2);
            let head_request = apply_request(
                &target,
                predecessor(Some("alpha-001"), 1, checksum(&migration)),
                2,
                submission(&next),
            );
            let error = run_apply(&db, head_request)
                .await
                .expect_err("an apply while the gate is held must be refused");
            assert_code(error, "schema_migration_in_progress");
            db.release_schema_migration_gate("testdb", 2);

            // The stale generation cannot EXECUTE either, even with nothing held:
            // the fence is free (the takeover released it after acquiring at
            // generation 2), so the
            // refusal of the stale runner's `expected_fence_generation = 1` comes
            // from the durable fence CAS, mapped to the stable in-progress code —
            // with no journal write and no executed effect.
            let pristine = journal_rows(&db).await;
            assert!(db.schema_migration_gate_generation("testdb").is_none());
            assert!(db
                .schema_migration_gate_in_doubt_generation("testdb")
                .is_none());
            let stale_request = apply_request(
                &target,
                predecessor(Some("alpha-001"), 1, checksum(&migration)),
                1,
                submission(&next),
            );
            let error = run_apply(&db, stale_request)
                .await
                .expect_err("a stale fence generation must not execute the next migration");
            assert_code(error, "schema_migration_in_progress");
            assert_eq!(
                journal_rows(&db).await,
                pristine,
                "the stale-generation refusal writes nothing"
            );
            let instance = db.get_db("testdb").unwrap();
            assert!(
                !instance.has_table("main", "items_2"),
                "the fenced-out runner must not have executed the next migration"
            );

            // Positive control: the CURRENT runner — generation derived from the
            // status (the durable fence head, released after the takeover) — applies the
            // same next migration and its effect exists.
            let status = run_status(&db, &target).await;
            let current = u64::try_from(
                field(&status, "fence_generation")
                    .as_i64()
                    .expect("the status reports a durable fence generation"),
            )
            .expect("the durable fence generation fits in u64");
            let fresh_request = apply_request(
                &target,
                predecessor(Some("alpha-001"), 1, checksum(&migration)),
                current,
                submission(&next),
            );
            let value = run_apply(&db, fresh_request)
                .await
                .expect("the status-derived current generation applies the next migration");
            assert_eq!(str_field(&value, "status"), "applied");
            let instance = db.get_db("testdb").unwrap();
            assert!(instance.has_table("main", "items_2"));
        });
        timeout(Duration::from_secs(180), scenario)
            .await
            .expect("restart scenario exceeded its 180 s whole-scenario bound");
        guard.mark("scenario done");
    });
}

#[tokio::test]
async fn changed_body_ordinal_or_id_resubmissions_are_refused_with_no_write() {
    let (_dir, db, _migration) = crash_started_only_and_reopen().await;
    let target = identity(&db).await;

    // The unresolved journal pins migration alpha-001 (ordinal 1). A changed
    // body (same id and ordinal) disagrees with the stored checksum...
    let redefined = SchemaMigration::new("alpha-001", 1, "migration 1 redefined")
        .op(ddl::create_table("different_table").build())
        .expect("typed create-table operation is admitted")
        .build()
        .expect("test migration body is valid");
    // ...a changed ordinal substitutes another ordinal...
    let reordinaled = body("alpha-001", 2);
    // ...and a changed id substitutes another migration.
    let renamed = body("alpha-002", 1);

    let pristine = journal_rows(&db).await;
    for (name, migration) in [
        ("changed body", redefined),
        ("changed ordinal", reordinaled),
        ("changed id", renamed),
    ] {
        let error = run_apply(
            &db,
            apply_request(&target, empty_predecessor(), 0, submission(&migration)),
        )
        .await
        .expect_err(&format!("the {name} resubmission must be refused"));
        let expected = match name {
            "changed body" => "schema_migration_checksum_mismatch",
            _ => "schema_migration_predecessor_mismatch",
        };
        assert_code(error, expected);
    }
    assert_eq!(
        journal_rows(&db).await,
        pristine,
        "the mismatch refusals write nothing"
    );
}
