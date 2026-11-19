//! Persisted control-record integrity regressions (R1c-4 follow-up P1,
//! design §4.2/§5.3): while a durable OWNED `Started` event remains, a
//! deleted, retyped, or drifted control record fails its own database
//! closed on status, plan and apply with
//! `schema_migration_durability_unavailable` in the same process, and on
//! the cold restart the open-time in-doubt seed reads the damaged
//! partition with the ordinary history reader — so the WHOLE startup
//! fails closed with the exact `DbError::SchemaMigrationJournalUnreadable`
//! and its stable code. There is no damaged-startup success and no
//! per-database quarantine (asserted by [`assert_restart_fails_closed`]
//! for every damage below).

use std::time::Duration;

use shamir_test_diag::FlakeGuard;

use super::super::helpers::on_big_stack;
use super::super::helpers::{
    body, context, fence_generation_from_status, find_journal_row, journal_table, persistent_db,
    predecessor, prepare_database, run_apply, run_plan, run_status, submission,
    try_init_waiting_for_locks, wal_delete_journal_row, wal_patch_journal_row,
    wal_replace_journal_row_without_key,
};
use crate::query::batch::BatchError;
use crate::shamir_db::system_store::DatabaseIdentity;
use crate::types::value::QueryValue;
use crate::ShamirDb;

/// Which persisted damage the scenario sows into ALPHA's control plane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Damage {
    /// ALPHA's whole control ROW is physically deleted; the owned `Started`
    /// event stays. A key-less (field-only) mutation would not exercise the
    /// physical lookup, so the row itself is removed.
    ControlRowDeleted,
    /// ALPHA's control row survives but its `record_type` is changed, so it
    /// no longer reads as a control record; the owned `Started` event stays.
    /// Routing still follows the KEY prefix, so the retyped row stays
    /// attributable to ALPHA (its own in-doubt partition), never BETA's.
    RecordTypeChanged,
    /// ALPHA's `Started` event row is deleted; the owned control row stays.
    StartedEventDeleted,
    /// ALPHA's control `sequence` is advanced past the journal tail.
    SequenceDrift,
    /// ALPHA's control `generation` is moved off the latest event's.
    GenerationDrift,
}

/// Which UNATTRIBUTABLE damage the startup-failure scenario sows: the row's
/// routing key is missing or points at another database's identity, so no
/// partition can own it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UnattributableDamage {
    /// ALPHA's control row key is re-pointed at BETA's canonical CONTROL key
    /// while BETA has no journal rows yet (so the key is unoccupied and the
    /// unique `record_key` index admits the write).
    ControlKeyPointsToForeignControl,
    /// ALPHA's started EVENT row key is re-pointed at BETA's canonical but
    /// UNUSED `event/{beta}/1` (BETA has no journal traffic).
    EventKeyPointsToForeignEvent,
    /// ALPHA's control row loses its `record_key` field entirely. The
    /// unique-index validator skips a record whose new key is absent, so
    /// the WAL-backed delete+insert replacement persists.
    ControlKeyDeleted,
    /// ALPHA's started event payload is rewritten IN PLACE so the
    /// serialized `JournalEvent.database_identity` names BETA while the
    /// row's envelope and key still route to ALPHA. Uniquely admitted: the
    /// `record_key` is untouched.
    EventPayloadIdentityContradictsKey,
}

/// A merge-patch set-map for the WAL-backed update seam.
fn patch(pairs: &[(&str, QueryValue)]) -> QueryValue {
    let mut fields = shamir_types::types::common::new_map();
    for (name, value) in pairs {
        fields.insert((*name).to_string(), value.clone());
    }
    QueryValue::Map(fields)
}

/// Sow the chosen damage through the WAL-BACKED implicit-transaction seams
/// ONLY (see `helpers.rs`): every mutation lands in one durable `WalEntryV2`
/// and therefore survives the cold restart below.
async fn sow_damage(db: &ShamirDb, damage: Damage, alpha: &DatabaseIdentity) {
    match damage {
        Damage::ControlRowDeleted => {
            // The control ROW itself is gone; the owned Started event row
            // remains readable, so the failure below is purely the missing
            // control record.
            let (_, key, _) = find_journal_row(db, alpha, "control").await;
            wal_delete_journal_row(db, &key).await;
        }
        Damage::RecordTypeChanged => {
            // Same physical row, no longer typed as a control record — an
            // identity-keyed read that matches on row shape alone must not
            // treat it as ALPHA's control plane.
            let (_, key, _) = find_journal_row(db, alpha, "control").await;
            wal_patch_journal_row(
                db,
                &key,
                patch(&[("record_type", QueryValue::Str("event".into()))]),
            )
            .await;
        }
        Damage::StartedEventDeleted => {
            let (_, key, _) = find_journal_row(db, alpha, "event").await;
            wal_delete_journal_row(db, &key).await;
        }
        Damage::SequenceDrift | Damage::GenerationDrift => {
            let field_name = match damage {
                Damage::SequenceDrift => "sequence",
                _ => "generation",
            };
            let (_, key, envelope) = find_journal_row(db, alpha, "control").await;
            let counter = envelope
                .get(field_name)
                .and_then(QueryValue::as_str)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_else(|| panic!("control {field_name} is a counter string"));
            wal_patch_journal_row(
                db,
                &key,
                patch(&[(field_name, QueryValue::Str((counter + 1).to_string()))]),
            )
            .await;
        }
    }
}

fn assert_unavailable(error: BatchError, scenario: &str) {
    assert_eq!(
        error.code(),
        Some("schema_migration_durability_unavailable"),
        "{scenario}: wrong refusal code; message: {error}"
    );
}

/// The three §5.3 application-schema operations of the damaged database all
/// fail closed with the same stable code.
async fn assert_alpha_fails_closed(db: &ShamirDb, alpha: &DatabaseIdentity, scenario: &str) {
    let status_error = run_status(db, "alpha", alpha)
        .await
        .expect_err(&format!("{scenario}: alpha status must fail closed"));
    assert_unavailable(status_error, &format!("{scenario}: status"));
    let plan_error = run_plan(
        db,
        "alpha",
        predecessor(None, 0, [0; 32]),
        vec![submission(&body("alpha-001", 1))],
        alpha,
    )
    .await
    .expect_err(&format!("{scenario}: alpha plan must fail closed"));
    assert_unavailable(plan_error, &format!("{scenario}: plan"));
    let apply_error = run_apply(
        db,
        "alpha",
        predecessor(None, 0, [0; 32]),
        0,
        submission(&body("alpha-001", 1)),
        alpha,
    )
    .await
    .expect_err(&format!("{scenario}: alpha apply must fail closed"));
    assert_unavailable(apply_error, &format!("{scenario}: apply"));
}

/// Shared damaged-restart oracle: flush the journal rows durably, drop the
/// engine's handles and REOPEN expecting FAILURE — the open-time in-doubt
/// seed reads every routed partition with the ordinary history reader, so
/// persisted damage (attributable or not) fails the WHOLE startup closed.
/// Asserts the exact `DbError::SchemaMigrationJournalUnreadable` variant
/// (not just any `durability_unavailable`-coded error) plus its stable
/// `.code()` and the scenario in the message; the bounded Locked-only
/// retry of [`try_init_waiting_for_locks`] still applies.
async fn assert_restart_fails_closed(dir: tempfile::TempDir, db: ShamirDb, scenario: &str) {
    let table = journal_table(&db).await;
    db.system_store()
        .flush_system_repo_durable(&table)
        .await
        .expect("the damaged journal rows flush durably before the restart");
    drop(table);
    drop(db);
    let Err(error) = try_init_waiting_for_locks(dir.path().join("system")).await else {
        panic!("{scenario}: startup must fail closed on the persisted damage");
    };
    assert_unavailable_error(&error, scenario);
    // Keep the tempdir alive to the end of the scenario.
    drop(dir);
}

/// Shared persisted scenario: two databases, each with journal traffic;
/// ALPHA left with a durable OWNED Started event; the chosen damage sown
/// into ALPHA's control plane. The pre-restart
/// oracle fails ALPHA's own status, plan and apply closed in the SAME
/// process; then the durable flush + cold restart fails the WHOLE startup
/// closed ([`assert_restart_fails_closed`]).
async fn control_integrity_scenario(damage: Damage) {
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

    // ALPHA: durable OWNED fence with one Started event — the in-flight
    // state a crashed runner leaves behind. Never applied, never released.
    let alpha_body = body("alpha-001", 1);
    db.system_store()
        .acquire_schema_migration_fence(
            alpha,
            0,
            alpha_body.metadata.clone(),
            alpha_body,
            false,
            &context(),
        )
        .await
        .unwrap();

    // BETA: one fully applied migration, so BETA owns a control row of its
    // own and the identity-conditioned lookup/CAS has a REAL neighbor row to
    // skip while reading ALPHA's damaged partition.
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
    .expect("database B applies its first migration in the clean setup");
    assert_eq!(
        applied.get("status").and_then(QueryValue::as_str),
        Some("applied"),
        "database B applies its first migration in the clean setup"
    );

    sow_damage(&db, damage, &alpha).await;

    // BEFORE the restart the damage — already committed through the
    // WAL-backed seam (`affected == 1` asserted there) — fails ALPHA's own
    // status, plan and apply closed in the SAME process; BETA's status stays
    // readable (its full apply flow lives in `isolation_tests`).
    let scenario = format!("pre-restart alpha with {damage:?}");
    assert_alpha_fails_closed(&db, &alpha, &scenario).await;
    run_status(&db, "beta", &beta)
        .await
        .expect("beta status stays readable beside ALPHA's attributable damage");

    // Persist the damaged rows durably, drop the engine, REOPEN: the
    // open-time in-doubt seed fails the WHOLE startup closed.
    let scenario = format!("restart with {damage:?}");
    assert_restart_fails_closed(dir, db, &scenario).await;
}

/// Sow the UNATTRIBUTABLE damage through the WAL-BACKED implicit-transaction
/// seams ONLY (see `helpers.rs`). Every target key is a canonical key of
/// BETA that is UNOCCUPIED (BETA has no journal traffic in this scenario),
/// so the unique `record_key` index admits the raw write at commit time.
async fn sow_unattributable_damage(
    db: &ShamirDb,
    damage: UnattributableDamage,
    alpha: &DatabaseIdentity,
    beta: &DatabaseIdentity,
) {
    let (record_key, envelope) = match damage {
        UnattributableDamage::EventKeyPointsToForeignEvent
        | UnattributableDamage::EventPayloadIdentityContradictsKey => {
            let (_, key, row) = find_journal_row(db, alpha, "event").await;
            (key, row)
        }
        _ => {
            let (_, key, row) = find_journal_row(db, alpha, "control").await;
            (key, row)
        }
    };
    match damage {
        UnattributableDamage::ControlKeyPointsToForeignControl => {
            wal_patch_journal_row(
                db,
                &record_key,
                patch(&[("record_key", QueryValue::Str(format!("control/{beta}")))]),
            )
            .await;
        }
        UnattributableDamage::EventKeyPointsToForeignEvent => {
            wal_patch_journal_row(
                db,
                &record_key,
                patch(&[("record_key", QueryValue::Str(format!("event/{beta}/1")))]),
            )
            .await;
        }
        UnattributableDamage::ControlKeyDeleted => {
            // Merge-patch cannot REMOVE a field: the key-less replacement is
            // one atomic WAL-backed delete+insert of the same envelope
            // without its `record_key`.
            let mut keyless = envelope;
            let QueryValue::Map(fields) = &mut keyless else {
                panic!("the control row envelope is a map");
            };
            assert!(
                fields.shift_remove("record_key").is_some(),
                "the control row carried a record_key before the damage",
            );
            wal_replace_journal_row_without_key(db, &record_key, keyless).await;
        }
        UnattributableDamage::EventPayloadIdentityContradictsKey => {
            // Envelope and key stay ALPHA-routed; only the payload's typed
            // `JournalEvent.database_identity` is mutated and re-serialized
            // in the actual named format — no generic field round-trip.
            let QueryValue::Bin(bytes) = envelope
                .get("event")
                .expect("event payload present")
                .clone()
            else {
                panic!("the event row payload is binary");
            };
            let mut event: crate::shamir_db::system_store::JournalEvent =
                rmp_serde::from_slice(&bytes).expect("started event payload decodes");
            event.database_identity = *beta;
            let mutated =
                rmp_serde::to_vec_named(&event).expect("mutated started event payload re-encodes");
            wal_patch_journal_row(
                db,
                &record_key,
                patch(&[("event", QueryValue::Bin(mutated))]),
            )
            .await;
        }
    }
}

/// Unattributable-row scenario: two databases, BETA with NO journal traffic
/// (its canonical keys stay unoccupied and writable); ALPHA with a durable
/// OWNED Started event; the chosen unattributable damage sown. BEFORE the
/// restart the damage splits: the routed payload disagreement (the event
/// envelope stays ALPHA-attributable, only its payload identity is
/// contradicted) keeps BETA healthy beside ALPHA's fail-closed status, while
/// the outer key defects leave the row unroutable and BOTH databases' status
/// fails closed. After the durable flush and a cold restart the WHOLE startup
/// fails closed with the typed `schema_migration_durability_unavailable`
/// (observed through `DbError::code()`, never message matching), using the
/// bounded Locked-only retry.
async fn unattributable_row_scenario(damage: UnattributableDamage) {
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

    let alpha_body = body("alpha-001", 1);
    db.system_store()
        .acquire_schema_migration_fence(
            alpha,
            0,
            alpha_body.metadata.clone(),
            alpha_body,
            false,
            &context(),
        )
        .await
        .unwrap();

    sow_unattributable_damage(&db, damage, &alpha, &beta).await;

    // BEFORE the restart: the OWNING request (ALPHA) always fails closed
    // with the typed code. For key/envelope-level damage the row is
    // unroutable, so BETA's ordinary read fails too; for the payload
    // contradiction the row still routes to ALPHA, so BETA stays healthy
    // (isolation) — only the open-time probe takes the whole store down.
    let alpha_error = run_status(&db, "alpha", &alpha)
        .await
        .expect_err("alpha status must fail closed on an unattributable row");
    assert_unavailable(
        alpha_error,
        &format!("pre-restart alpha status with {damage:?}"),
    );
    match damage {
        UnattributableDamage::EventPayloadIdentityContradictsKey => {
            run_status(&db, "beta", &beta)
                .await
                .expect("beta status stays healthy pre-restart beside the contradicted payload");
        }
        _ => {
            let beta_error = run_status(&db, "beta", &beta)
                .await
                .expect_err("beta status must fail closed on an unroutable row");
            assert_unavailable(beta_error, "pre-restart beta status");
        }
    }

    // The routed payload disagreement keeps BETA healthy pre-restart
    // (isolation); the shared helper then proves the cold restart fails the
    // WHOLE startup closed.
    let scenario = format!("restart with {damage:?}");
    assert_restart_fails_closed(dir, db, &scenario).await;
}

/// The DbError-side twin of [`assert_unavailable`]: the EXACT
/// `DbError::SchemaMigrationJournalUnreadable` variant (not just any
/// `durability_unavailable`-coded error), its stable `.code()`, and the
/// scenario in the message.
fn assert_unavailable_error(error: &crate::DbError, scenario: &str) {
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

/// Whole-scenario bound (>=120 s: the Windows runner is up to 10x slower)
/// with the failure-only diagnostic watchdog: it fires BELOW the runner's
/// slow-timeout kill so the timeline dumps first.
fn control_integrity_test(name: &'static str, damage: Damage) {
    on_big_stack(move || async move {
        let guard = FlakeGuard::with_watchdog(name, Duration::from_secs(150));
        let scenario = Box::pin(control_integrity_scenario(damage));
        tokio::time::timeout(Duration::from_secs(300), scenario)
            .await
            .expect("control integrity scenario exceeded its 300 s bound");
        guard.mark("scenario done");
    });
}

/// Same bound for the unattributable-row startup-failure scenarios.
fn unattributable_test(name: &'static str, damage: UnattributableDamage) {
    on_big_stack(move || async move {
        let guard = FlakeGuard::with_watchdog(name, Duration::from_secs(150));
        let scenario = Box::pin(unattributable_row_scenario(damage));
        tokio::time::timeout(Duration::from_secs(300), scenario)
            .await
            .expect("unattributable-row scenario exceeded its 300 s bound");
        guard.mark("scenario done");
    });
}

#[test]
fn physically_deleted_control_row_fails_the_database_closed_with_started_event_remaining() {
    control_integrity_test(
        "physically_deleted_control_row_fails_the_database_closed_with_started_event_remaining",
        Damage::ControlRowDeleted,
    );
}

#[test]
fn retyped_control_row_fails_the_database_closed_with_started_event_remaining() {
    control_integrity_test(
        "retyped_control_row_fails_the_database_closed_with_started_event_remaining",
        Damage::RecordTypeChanged,
    );
}

#[test]
fn deleted_control_record_key_fails_startup_closed_as_unattributable() {
    unattributable_test(
        "deleted_control_record_key_fails_startup_closed_as_unattributable",
        UnattributableDamage::ControlKeyDeleted,
    );
}

#[test]
fn control_key_pointing_at_another_database_fails_startup_closed_as_unattributable() {
    unattributable_test(
        "control_key_pointing_at_another_database_fails_startup_closed_as_unattributable",
        UnattributableDamage::ControlKeyPointsToForeignControl,
    );
}

#[test]
fn event_key_pointing_at_another_database_fails_startup_closed_as_unattributable() {
    unattributable_test(
        "event_key_pointing_at_another_database_fails_startup_closed_as_unattributable",
        UnattributableDamage::EventKeyPointsToForeignEvent,
    );
}

#[test]
fn started_event_payload_identity_contradicting_its_key_fails_startup_closed_as_unattributable() {
    unattributable_test(
        "started_event_payload_identity_contradicting_its_key_fails_startup_closed_as_unattributable",
        UnattributableDamage::EventPayloadIdentityContradictsKey,
    );
}

#[test]
fn deleted_started_event_row_fails_the_owned_fence_closed_after_a_restart() {
    control_integrity_test(
        "deleted_started_event_row_fails_the_owned_fence_closed_after_a_restart",
        Damage::StartedEventDeleted,
    );
}

#[test]
fn control_sequence_drift_fails_the_database_closed_after_a_restart() {
    control_integrity_test(
        "control_sequence_drift_fails_the_database_closed_after_a_restart",
        Damage::SequenceDrift,
    );
}

#[test]
fn control_generation_drift_fails_the_database_closed_after_a_restart() {
    control_integrity_test(
        "control_generation_drift_fails_the_database_closed_after_a_restart",
        Damage::GenerationDrift,
    );
}
