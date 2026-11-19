//! Persisted control-record integrity regressions (R1c-4 follow-up P1,
//! design §4.2/§5.3): while a durable OWNED `Started` event remains, a
//! deleted, retyped, or drifted control record fails its own database
//! closed on status, plan and apply with
//! `schema_migration_durability_unavailable` (the open-time seed cannot
//! establish ownership, so the unknown-generation in-doubt marker stands
//! and ordinary DML, DDL and stored-procedure calls are refused with
//! `schema_migration_in_doubt`), while a healthy database on the same
//! journal table keeps its full status/plan/actual-apply path.
//!
//! UNATTRIBUTABLE damage — a missing routing key, or a key whose identity
//! contradicts the row's `database_identity` — is quarantined by NOBODY:
//! the one-shot open-time routing scan fails the WHOLE startup closed with
//! the typed `schema_migration_durability_unavailable`
//! (`DbError::SchemaMigrationJournalUnreadable` surfaced through `code()`;
//! covered in `unattributable_*` below). The sowing seams use
//! only keys the unique `record_key` index admits: B's colliding CONTROL
//! key is already occupied once B has journal traffic (the physical
//! `TableManager::set` validates unique indexes before write, so the old
//! colliding-with-B's-LIVE-key variant was unreachable and is gone); the
//! scenarios below instead target B's UNUSED canonical keys while B has no
//! journal rows.
//!
//! The control row is damaged ONLY through the existing raw test seams
//! (`rows`/`overwrite_row`/`TableManager::delete`); every request is built
//! through `shamir-query-builder`. The per-database variants are PERSISTED:
//! the damaged row is flushed durably and the store is reopened through the
//! bounded lock-only retry before any assertion, so none of the covered
//! cases can pass by a before-fix in-memory accident.

use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::{ddl, write};
use shamir_test_diag::FlakeGuard;

use super::super::helpers::on_big_stack;
use super::super::helpers::{
    body, checksum, context, fence_generation_from_status, field, flush_and_reopen, journal_table,
    overwrite_row, persistent_db, predecessor, prepare_database, rows, run_admin_op, run_apply,
    run_plan, run_status, str_field, submission, try_init_waiting_for_locks, OWNER,
};
use crate::engine::table::TableManager;
use crate::query::batch::BatchError;
use crate::shamir_db::system_store::DatabaseIdentity;
use crate::types::value::QueryValue;
use crate::ShamirDb;
use shamir_types::types::record_id::RecordId;

/// Which persisted damage the scenario sows into ALPHA's control plane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Damage {
    /// ALPHA's whole control ROW is physically deleted; the owned `Started`
    /// event stays. A key-less (field-only) mutation would not exercise the
    /// physical lookup, so the row itself is removed.
    ControlRowDeleted,
    /// ALPHA's control row survives but its `record_type` is changed, so it
    /// no longer reads as a control record; the owned `Started` event stays.
    /// Routing still follows the KEY prefix, so the row stays attributable
    /// to ALPHA (its own quarantine), never BETA's.
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
    /// unique-index validator skips a record whose new key is absent, so the
    /// write persists (and drops the old posting).
    ControlKeyDeleted,
    /// ALPHA's started event payload is rewritten IN PLACE (raw seam) so the
    /// serialized `JournalEvent.database_identity` names BETA while the row's
    /// envelope and key still route to ALPHA. Uniquely admitted: the
    /// `record_key` is untouched. Readable payload identity contradicting
    /// the routed key proves no ownership — whole-open typed failure, while
    /// the ordinary reader keeps BETA isolated until open.
    EventPayloadIdentityContradictsKey,
}

/// The persisted system row of ALPHA's control record, for raw damage.
async fn alpha_control_row(db: &ShamirDb, alpha: &DatabaseIdentity) -> (RecordId, QueryValue) {
    alpha_row(db, alpha, "control").await
}

/// The persisted system row of ALPHA's started EVENT record, for raw damage.
async fn alpha_event_row(db: &ShamirDb, alpha: &DatabaseIdentity) -> (RecordId, QueryValue) {
    alpha_row(db, alpha, "event").await
}

async fn alpha_row(
    db: &ShamirDb,
    alpha: &DatabaseIdentity,
    record_type: &str,
) -> (RecordId, QueryValue) {
    let table = journal_table(db).await;
    rows(&table)
        .await
        .into_iter()
        .find(|(_, row)| {
            row.get("record_type").and_then(QueryValue::as_str) == Some(record_type)
                && row.get("database_identity").and_then(QueryValue::as_str)
                    == Some(alpha.to_string().as_str())
        })
        .unwrap_or_else(|| panic!("alpha {record_type} row is persisted"))
}

/// Sow the chosen damage with the raw test seams ONLY.
async fn sow_damage(db: &ShamirDb, damage: Damage, alpha: &DatabaseIdentity) {
    match damage {
        Damage::ControlRowDeleted => {
            // The control ROW itself is gone; the owned Started event row
            // remains readable, so the failure below is purely the missing
            // control record.
            let (event_id, _) = alpha_control_row(db, alpha).await;
            let table = journal_table(db).await;
            assert!(
                table
                    .delete(event_id)
                    .await
                    .expect("raw test deletion of the control row returns success",),
                "physical deletion must report the control row existed",
            );
        }
        Damage::RecordTypeChanged => {
            // Same physical row, no longer typed as a control record — an
            // identity-keyed read that matches on row shape alone must not
            // treat it as ALPHA's control plane.
            let (event_id, mut envelope) = alpha_control_row(db, alpha).await;
            let QueryValue::Map(fields) = &mut envelope else {
                panic!("the control row envelope is a map");
            };
            fields.insert("record_type".into(), QueryValue::Str("event".into()));
            let table = journal_table(db).await;
            overwrite_row(&table, event_id, envelope).await;
        }
        Damage::StartedEventDeleted => {
            let table = journal_table(db).await;
            let (event_id, _) = rows(&table)
                .await
                .into_iter()
                .find(|(_, row)| {
                    row.get("record_type").and_then(QueryValue::as_str) == Some("event")
                        && row.get("database_identity").and_then(QueryValue::as_str)
                            == Some(alpha.to_string().as_str())
                })
                .expect("alpha started event row is persisted");
            assert!(
                table
                    .delete(event_id)
                    .await
                    .expect("raw test deletion of the started event row returns success"),
                "physical deletion must report the event row existed"
            );
        }
        Damage::SequenceDrift => {
            let (event_id, mut envelope) = alpha_control_row(db, alpha).await;
            let QueryValue::Map(fields) = &mut envelope else {
                panic!("the control row envelope is a map");
            };
            let sequence = fields
                .get("sequence")
                .and_then(QueryValue::as_str)
                .and_then(|value| value.parse::<u64>().ok())
                .expect("control sequence is a counter string");
            fields.insert(
                "sequence".into(),
                QueryValue::Str((sequence + 1).to_string()),
            );
            let table = journal_table(db).await;
            overwrite_row(&table, event_id, envelope).await;
        }
        Damage::GenerationDrift => {
            let (event_id, mut envelope) = alpha_control_row(db, alpha).await;
            let QueryValue::Map(fields) = &mut envelope else {
                panic!("the control row envelope is a map");
            };
            let generation = fields
                .get("generation")
                .and_then(QueryValue::as_str)
                .and_then(|value| value.parse::<u64>().ok())
                .expect("control generation is a counter string");
            fields.insert(
                "generation".into(),
                QueryValue::Str((generation + 1).to_string()),
            );
            let table = journal_table(db).await;
            overwrite_row(&table, event_id, envelope).await;
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

/// One ordinary DML, one DDL and one stored-procedure call batch against the
/// damaged database: each is refused typed in-doubt by the batch fence,
/// never executed. The DML targets the REAL `sentinel_items` table created
/// in the clean setup — the refusal must not have touched it.
async fn assert_batches_refused_in_doubt(db: &ShamirDb, scenario: &str) {
    let mut write_batch = Batch::new();
    write_batch.insert(
        "w",
        write::insert("sentinel_items")
            .row(write::doc().set("id", 1i64).expect("typed doc field"))
            .build(),
    );
    let request = write_batch.to_request_via_msgpack().unwrap();
    let error = db
        .execute_as(OWNER, "alpha", &request)
        .await
        .expect_err(&format!(
            "{scenario}: ordinary DML must be refused in doubt"
        ));
    assert_eq!(
        error.code(),
        Some("schema_migration_in_doubt"),
        "{scenario}: wrong DML refusal code; message: {error}"
    );

    let mut ddl_batch = Batch::new();
    ddl_batch.op(
        "ct",
        ddl::create_table("blocked_items").repo("main").build(),
    );
    let request = ddl_batch.to_request_via_msgpack().unwrap();
    let error = db
        .execute_as(OWNER, "alpha", &request)
        .await
        .expect_err(&format!("{scenario}: DDL must be refused in doubt"));
    assert_eq!(
        error.code(),
        Some("schema_migration_in_doubt"),
        "{scenario}: wrong DDL refusal code; message: {error}"
    );

    let mut call_batch = Batch::new();
    call_batch.call("proc", "blocked_procedure", [] as [i64; 0]);
    let request = call_batch.to_request_via_msgpack().unwrap();
    let error = db
        .execute_as(OWNER, "alpha", &request)
        .await
        .expect_err(&format!("{scenario}: call must be refused in doubt"));
    assert_eq!(
        error.code(),
        Some("schema_migration_in_doubt"),
        "{scenario}: wrong call refusal code; message: {error}"
    );

    // The sentinel table survives and stays EMPTY: the refused DML inserted
    // no row (a surviving table alone would not prove that, so the rows are
    // counted), and the refused DDL created nothing beside it.
    let instance = db.get_db("alpha").unwrap();
    assert!(
        instance.has_table("main", "sentinel_items"),
        "{scenario}: the real sentinel table must survive untouched"
    );
    assert!(
        !instance.has_table("main", "blocked_items"),
        "{scenario}: the refused DDL must not have executed"
    );
    let sentinel = db
        .get_table("alpha", "main", "sentinel_items")
        .await
        .expect("the sentinel table is readable after the refusals");
    assert_eq!(
        rows(&sentinel).await.len(),
        0,
        "{scenario}: the refused DML must not have inserted any sentinel row"
    );
}

/// The healthy database keeps its full path across the damaged neighbor's
/// failures: status reports its applied prefix, plan of the next ordinal
/// works, and an ACTUAL apply lands — through the identity-conditioned
/// control lookup and MVCC CAS. The apply generation is taken from B's OWN
/// status, never a magic constant.
async fn assert_beta_stays_healthy(
    db: &ShamirDb,
    beta: &DatabaseIdentity,
    beta_first: &shamir_query_types::admin::types::schema_migration::SchemaMigrationBody,
) {
    let beta_next = body("beta-002", 2);
    let status = run_status(db, "beta", beta)
        .await
        .expect("database B status stays healthy beside the damaged A control row");
    assert_eq!(str_field(&status, "last_applied_id"), "beta-001");
    assert_eq!(field(&status, "last_applied_ordinal").as_i64(), Some(1));
    assert_eq!(str_field(&status, "checksum_validation"), "valid");
    let planned = run_plan(
        db,
        "beta",
        predecessor(Some("beta-001"), 1, checksum(beta_first)),
        vec![submission(beta_first), submission(&beta_next)],
        beta,
    )
    .await
    .expect("database B plans its next migration beside the damaged A control row");
    assert_eq!(str_field(field(&planned, "migration"), "id"), "beta-002");
    let applied = run_apply(
        db,
        "beta",
        predecessor(Some("beta-001"), 1, checksum(beta_first)),
        fence_generation_from_status(db, "beta", beta).await,
        submission(&beta_next),
        beta,
    )
    .await
    .expect("database B actually applies its second migration beside the damaged A control row");
    assert_eq!(str_field(&applied, "status"), "applied");
    assert_eq!(str_field(&applied, "migration_id"), "beta-002");
    let status_after = run_status(db, "beta", beta)
        .await
        .expect("database B status after its own apply stays healthy");
    assert_eq!(str_field(&status_after, "last_applied_id"), "beta-002");
}

/// Shared persisted scenario: two databases, each with journal traffic;
/// ALPHA left with a durable OWNED Started event and a REAL sentinel table;
/// the chosen damage sown into ALPHA's control plane; durable flush; cold
/// restart; then the fail-closed, refused-DML/DDL/call and healthy-neighbor
/// assertions.
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

    // A REAL table sentinel in ALPHA, BEFORE any fence: the in-doubt DML/DDL
    // refusals below must leave it untouched.
    let mut sentinel_batch = Batch::new();
    sentinel_batch.op(
        "ct",
        ddl::create_table("sentinel_items").repo("main").build(),
    );
    let request = sentinel_batch.to_request_via_msgpack().unwrap();
    db.execute_as(OWNER, "alpha", &request)
        .await
        .expect("the sentinel create-table runs before the fence exists");

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
    assert_eq!(str_field(&applied, "status"), "applied");

    sow_damage(&db, damage, &alpha).await;

    // Persist the damaged rows durably, drop the engine, REOPEN: the
    // assertions below must hold across a cold restart of the store.
    let table: TableManager = journal_table(&db).await;
    let db = flush_and_reopen(dir.path(), db, table).await;

    let scenario = format!("restart with {damage:?}");
    assert_alpha_fails_closed(&db, &alpha, &scenario).await;
    assert_batches_refused_in_doubt(&db, &scenario).await;
    assert_beta_stays_healthy(&db, &beta, &beta_first).await;

    // Physical-delete variant: after boot, the healthy serving database B
    // must not be able to rename or cascade-drop the quarantined A —
    // database-level admin ops authorize at Root, but the target-side guard
    // refuses on A's persisted in-doubt marker. Root principal, as every
    // database-level admin op requires.
    if damage == Damage::ControlRowDeleted {
        let rename_error =
            run_admin_op(&db, "beta", "rn", ddl::rename_db("alpha", "alpha2").build())
                .await
                .expect_err("rename of a quarantined database must be refused");
        assert_eq!(
            rename_error.code(),
            Some("schema_migration_in_doubt"),
            "rename via healthy serving B: wrong refusal code; message: {rename_error}"
        );
        assert!(!db.has_db("alpha2"), "no shadow rename target may appear");

        let drop_error = run_admin_op(
            &db,
            "beta",
            "dd",
            ddl::drop_db("alpha")
                .hmac("test-hmac")
                .cascade()
                .build()
                .expect("typed drop-db operation"),
        )
        .await
        .expect_err("cascade drop of a quarantined database must be refused");
        assert_eq!(
            drop_error.code(),
            Some("schema_migration_in_doubt"),
            "cascade drop via healthy serving B: wrong refusal code; message: {drop_error}"
        );
        assert!(db.has_db("alpha"), "alpha must survive the refused drop");
    }
}

/// Sow the UNATTRIBUTABLE damage with the raw test seams ONLY. Every target
/// key is a canonical key of BETA that is UNOCCUPIED (BETA has no journal
/// traffic in this scenario), so the unique `record_key` index admits the
/// raw write.
async fn sow_unattributable_damage(
    db: &ShamirDb,
    damage: UnattributableDamage,
    alpha: &DatabaseIdentity,
    beta: &DatabaseIdentity,
) {
    let (row_id, mut envelope) = match damage {
        UnattributableDamage::EventKeyPointsToForeignEvent
        | UnattributableDamage::EventPayloadIdentityContradictsKey => {
            alpha_event_row(db, alpha).await
        }
        _ => alpha_control_row(db, alpha).await,
    };
    let QueryValue::Map(fields) = &mut envelope else {
        panic!("the journal row envelope is a map");
    };
    match damage {
        UnattributableDamage::ControlKeyPointsToForeignControl => {
            fields.insert(
                "record_key".into(),
                QueryValue::Str(format!("control/{beta}")),
            );
        }
        UnattributableDamage::EventKeyPointsToForeignEvent => {
            fields.insert(
                "record_key".into(),
                QueryValue::Str(format!("event/{beta}/1")),
            );
        }
        UnattributableDamage::ControlKeyDeleted => {
            assert!(
                fields.shift_remove("record_key").is_some(),
                "the control row carried a record_key before the damage",
            );
        }
        UnattributableDamage::EventPayloadIdentityContradictsKey => {
            // Envelope and key stay ALPHA-routed; only the payload's typed
            // `JournalEvent.database_identity` is mutated and re-serialized
            // in the actual named format — no generic field round-trip.
            let QueryValue::Bin(bytes) =
                fields.get("event").expect("event payload present").clone()
            else {
                panic!("the event row payload is binary");
            };
            let mut event: crate::shamir_db::system_store::JournalEvent =
                rmp_serde::from_slice(&bytes).expect("started event payload decodes");
            event.database_identity = *beta;
            let mutated =
                rmp_serde::to_vec_named(&event).expect("mutated started event payload re-encodes");
            fields.insert("event".into(), QueryValue::Bin(mutated));
        }
    }
    let table = journal_table(db).await;
    overwrite_row(&table, row_id, envelope).await;
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

    let table: TableManager = journal_table(&db).await;
    let system_path = dir.path().join("system");
    db.system_store()
        .flush_system_repo_durable(&table)
        .await
        .expect("the damaged journal rows flush durably before the restart");
    drop(table);
    drop(db);
    let Err(error) = try_init_waiting_for_locks(system_path).await else {
        panic!("startup must fail closed on an unattributable journal row");
    };
    assert_unavailable_error(&error, &format!("restart with {damage:?}"));
    // Keep the tempdir alive to the end of the scenario.
    drop(dir);
}

/// The DbError-side twin of [`assert_unavailable`]: typed code, not message
/// matching.
fn assert_unavailable_error(error: &crate::DbError, scenario: &str) {
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
