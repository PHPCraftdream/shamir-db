//! Server-side tests for the FENCING of application-schema APPLY (R1c step
//! 2b-3a): concurrent apply of the same ordinal (deterministic — no timing
//! dependence) and the maintenance fence refused to other sessions' writes,
//! DDL and stored-procedure calls while reads stay available.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::write::{doc, insert};
use shamir_query_builder::Query;

use super::super::helpers::body;
use super::helpers::{
    apply_request, assert_code, empty_predecessor, identity, persistent_owner_db, run_apply,
    str_field, submission,
};
use crate::shamir_db::system_store::JournalEventKind;
use shamir_query_types::filter::FilterValue;

#[tokio::test]
async fn concurrent_apply_of_the_same_ordinal_lets_exactly_one_migration_run() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = body("alpha-001", 1);

    // Two deterministic tasks race for ordinal 1: whichever arrives second
    // is refused by the in-process gate, the durable journal, or the fence
    // CAS — all of which map to `schema_migration_in_progress` — or, if the
    // first has already finished, sees the applied no-op. No outcome depends
    // on timing.
    let first = db.clone();
    let second = db.clone();
    let (left, right) = tokio::join!(
        run_apply(
            &first,
            apply_request(&target, empty_predecessor(), 0, submission(&migration)),
        ),
        run_apply(
            &second,
            apply_request(&target, empty_predecessor(), 0, submission(&migration)),
        ),
    );

    let mut applied_results = 0;
    for outcome in [left, right] {
        match outcome {
            Ok(value) => {
                assert_eq!(str_field(&value, "status"), "applied");
                applied_results += 1;
            }
            Err(error) => assert_code(error, "schema_migration_in_progress"),
        }
    }
    assert!(
        applied_results >= 1,
        "at least one task must apply the migration"
    );

    // Exactly one runner: the journal holds ONE migration run (5 events for
    // the single-operation body), applied once, fence released.
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    assert_eq!(history.events.len(), 5, "exactly one runner: {history:?}");
    assert_eq!(history.events[0].kind, JournalEventKind::Started);
    assert_eq!(
        history.events.last().unwrap().kind,
        JournalEventKind::FenceReleased
    );
    assert_eq!(history.applied.len(), 1);
    assert!(history.unresolved.is_empty());
    assert!(db.schema_migration_gate_generation("testdb").is_none());
}

#[tokio::test]
async fn a_held_fence_refuses_other_sessions_writes_ddl_and_calls_but_admits_reads_until_released()
{
    let (_dir, db) = persistent_owner_db().await;

    // The target table exists before the fence, so the read probe succeeds.
    let mut setup = Batch::new();
    setup.op("ct", ddl::create_table("items_1").build());
    db.execute_as(
        super::helpers::OWNER,
        "testdb",
        &setup.to_request_via_msgpack().unwrap(),
    )
    .await
    .unwrap();

    // Hold the in-process maintenance fence exactly as a live apply does.
    db.hold_schema_migration_gate("testdb", 1);

    let write_batch = || {
        let mut b = Batch::new();
        b.op(
            "ins",
            insert("items_1").row(
                doc()
                    .set("name", "fenced-out")
                    .expect("literal doc field interns"),
            ),
        );
        b
    };
    let ddl_batch = || {
        let mut b = Batch::new();
        b.op("ct", ddl::create_table("blocked").build());
        b
    };
    let call_batch = || {
        let mut b = Batch::new();
        b.call("c", "no_such_function", Vec::<FilterValue>::new());
        b
    };

    // Writes, DDL and stored-procedure calls from another session are
    // refused with the stable code.
    for (alias, batch) in [
        ("write", write_batch()),
        ("ddl", ddl_batch()),
        ("call", call_batch()),
    ] {
        let error = db
            .execute_as(
                super::helpers::OWNER,
                "testdb",
                &batch.to_request_via_msgpack().unwrap(),
            )
            .await
            .expect_err(&format!("{alias} must be refused during the fence"));
        assert_code(error, "schema_migration_in_progress");
    }

    // Reads stay available during the fence.
    let mut read = Batch::new();
    read.op("read", Query::from("items_1"));
    db.execute_as(
        super::helpers::OWNER,
        "testdb",
        &read.to_request_via_msgpack().unwrap(),
    )
    .await
    .expect("reads stay available during the fence");

    // Release: the same write is admitted, and so is the DDL.
    db.release_schema_migration_gate("testdb", 1);
    db.execute_as(
        super::helpers::OWNER,
        "testdb",
        &write_batch().to_request_via_msgpack().unwrap(),
    )
    .await
    .expect("the same write is admitted after the fence is released");
    db.execute_as(
        super::helpers::OWNER,
        "testdb",
        &ddl_batch().to_request_via_msgpack().unwrap(),
    )
    .await
    .expect("DDL is admitted after the fence is released");
}

/// The in-process gate is RESERVED before the durable fence CAS: exactly one
/// reservation exists at a time, it is visible to every other applier (and to
/// the batch entries) from that moment, and an early drop (CAS failure or a
/// dropped apply future) cancels it.
#[tokio::test]
async fn the_gate_reservation_is_exclusive_and_an_early_drop_cancels_it() {
    let (_dir, db) = persistent_owner_db().await;

    let first = db
        .reserve_schema_migration_gate("testdb")
        .expect("the first reservation wins");
    assert!(
        db.reserve_schema_migration_gate("testdb").is_none(),
        "a second applier must be refused while the gate is reserved"
    );
    assert!(
        db.schema_migration_gate_generation("testdb").is_some(),
        "a reserved gate is visible to every other applier"
    );

    drop(first);
    assert!(
        db.schema_migration_gate_generation("testdb").is_none(),
        "dropping the reservation before the hold cancels it"
    );
    assert!(
        db.reserve_schema_migration_gate("testdb").is_some(),
        "the gate is free again after the cancellation"
    );
}

/// After the hold the guard owns the live gate: a successful run releases it,
/// any other exit leaves the in-doubt marker (writes stay refused until a
/// resume), and neither exit evicts a newer owner's gate.
#[tokio::test]
async fn the_gate_guard_releases_on_success_and_leaves_an_in_doubt_marker_otherwise() {
    let (_dir, db) = persistent_owner_db().await;

    let mut succeeded = db.reserve_schema_migration_gate("testdb").unwrap();
    succeeded.hold(3);
    assert_eq!(db.schema_migration_gate_generation("testdb"), Some(3));
    succeeded.mark_succeeded();
    drop(succeeded);
    assert!(db.schema_migration_gate_generation("testdb").is_none());
    assert!(db
        .schema_migration_gate_in_doubt_generation("testdb")
        .is_none());

    let mut failed = db.reserve_schema_migration_gate("testdb").unwrap();
    failed.hold(4);
    drop(failed);
    assert!(db.schema_migration_gate_generation("testdb").is_none());
    assert_eq!(
        db.schema_migration_gate_in_doubt_generation("testdb"),
        Some(4),
        "an unsuccessful exit leaves the fence in doubt"
    );
}

/// Many appliers released together for the same ordinal: whatever their
/// interleaving, exactly one run executes (one `Started`, no takeover of a
/// live runner), the others are refused as in progress or see the applied
/// no-op, and no in-process gate or in-doubt marker is left behind.
#[tokio::test]
async fn eight_concurrent_appliers_run_the_migration_exactly_once() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = body("alpha-001", 1);

    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(8));
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let db = db.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            let request = apply_request(&target, empty_predecessor(), 0, submission(&migration));
            tokio::spawn(async move {
                barrier.wait().await;
                run_apply(&db, request).await
            })
        })
        .collect();

    let mut applied_results = 0;
    for task in tasks {
        match task.await.expect("applier task must not panic") {
            Ok(value) => {
                assert_eq!(str_field(&value, "status"), "applied");
                applied_results += 1;
            }
            Err(error) => assert_code(error, "schema_migration_in_progress"),
        }
    }
    assert!(applied_results >= 1, "one applier must apply the migration");

    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    let kinds: Vec<_> = history.events.iter().map(|event| event.kind).collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == JournalEventKind::Started)
            .count(),
        1,
        "exactly one runner started: {kinds:?}"
    );
    assert!(
        !kinds.contains(&JournalEventKind::RecoveryAcquired),
        "a live runner must never be taken over: {kinds:?}"
    );
    assert_eq!(
        history.events.len(),
        5,
        "one run of a single-operation body: {kinds:?}"
    );
    assert_eq!(history.applied.len(), 1);
    assert!(history.unresolved.is_empty());
    assert!(db.schema_migration_gate_generation("testdb").is_none());
    assert!(db
        .schema_migration_gate_in_doubt_generation("testdb")
        .is_none());
}
