//! Lifecycle refusals for a database in the HEALTHY UNRESOLVED IN-DOUBT
//! state (R1c-4 follow-up P1, design §5.3): with a durable OWNED `Started`
//! event and an open-time in-doubt marker standing after a cold restart,
//! database-level admin DDL — rename via a healthy session database, rename
//! via the in-doubt session itself, CASCADE drop, plain drop, and create
//! (no-op and typed `exists` refusal) — is refused with
//! `schema_migration_in_doubt` before any mutation, the stable identity and
//! healthy unresolved journal survive intact, and a healthy neighbor keeps
//! its full status/plan/actual-apply path.
//!
//! Database-level admin DDL authorizes at `ResourcePath::Root`, so every
//! admin op runs through the root principal (`db.execute` =
//! `Actor::System`); the ordinary owner principal (`User(7)`) has no root
//! grant. Application-schema applies run under the owner principal, as in
//! production.

use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationApplyRequest, SchemaMigrationBody,
};
use shamir_test_diag::FlakeGuard;

use super::super::helpers::on_big_stack;
use super::super::helpers::{
    body, checksum, context, fence_generation_from_status, field, flush_and_reopen, journal_table,
    persistent_db, predecessor, prepare_database, run_admin_op, run_apply, run_plan, run_status,
    str_field, submission, target, OWNER,
};
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::query::batch::BatchError;
use crate::shamir_db::system_store::{DatabaseIdentity, JournalEventKind};
use crate::ShamirDb;

fn assert_in_doubt(error: BatchError, context: &str) {
    assert_eq!(
        error.code(),
        Some("schema_migration_in_doubt"),
        "{context}: wrong refusal code; message: {error}"
    );
}

/// ALPHA is still exactly as the refusals left it: same stable identity,
/// healthy unresolved owned journal intact under that identity, sentinel
/// table present, marker still refusing an ordinary write.
async fn assert_alpha_intact(db: &ShamirDb, alpha: &DatabaseIdentity, context: &str) {
    let identity_now = db
        .system_store()
        .schema_migration_identity("alpha")
        .await
        .expect("alpha still resolves its stable identity");
    assert_eq!(identity_now, *alpha, "{context}: alpha identity replaced");
    let history = db
        .system_store()
        .read_schema_migration_history(identity_now)
        .await
        .expect("alpha's unresolved journal stays readable under the same identity");
    assert!(
        !history.unresolved.is_empty(),
        "{context}: the owned in-flight journal must survive the refused lifecycle"
    );
    let instance = db.get_db("alpha").expect("alpha still serves");
    assert!(
        instance.has_table("main", "sentinel_items"),
        "{context}: the real table sentinel must survive untouched"
    );
    // The in-doubt state still refuses an ordinary write on ALPHA: nothing
    // in the refused lifecycle operations cleared the marker.
    let mut b = Batch::new();
    b.insert(
        "w",
        shamir_query_builder::write::insert("sentinel_items")
            .row(
                shamir_query_builder::write::doc()
                    .set("id", 1i64)
                    .expect("typed doc field"),
            )
            .build(),
    );
    let request = b.to_request_via_msgpack().unwrap();
    let error = db
        .execute_as(OWNER, "alpha", &request)
        .await
        .expect_err(&format!("{context}: the marker must still refuse a write"));
    assert_in_doubt(error, &format!("{context}: marker still stands"));
}

/// The healthy database keeps its full path: status, plan, ACTUAL apply —
/// the apply generation derived from BETA's own status.
async fn assert_beta_stays_healthy(
    db: &ShamirDb,
    beta: &DatabaseIdentity,
    beta_first: &SchemaMigrationBody,
) {
    let beta_next = body("beta-002", 2);
    let status = run_status(db, "beta", beta)
        .await
        .expect("database B status stays healthy through ALPHA's lifecycle refusals");
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
    .expect("database B plans through ALPHA's lifecycle refusals");
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
    .expect("database B actually applies through ALPHA's lifecycle refusals");
    assert_eq!(str_field(&applied, "status"), "applied");
    assert_eq!(str_field(&applied, "migration_id"), "beta-002");
}

/// Target-side invariants that hold in EVERY fence state (reserved gate,
/// held gate, marker, free): the stable identity still resolves under
/// `name`, and the sentinel table survives.
async fn assert_alpha_target_intact(
    db: &ShamirDb,
    alpha: &DatabaseIdentity,
    name: &str,
    context: &str,
) {
    let identity_now = db
        .system_store()
        .schema_migration_identity(name)
        .await
        .unwrap_or_else(|e| panic!("{context}: '{name}' still resolves its identity: {e}"));
    assert_eq!(
        identity_now, *alpha,
        "{context}: '{name}' identity replaced"
    );
    let instance = db
        .get_db(name)
        .unwrap_or_else(|| panic!("{context}: '{name}' still serves"));
    assert!(
        instance.has_table("main", "sentinel_items"),
        "{context}: the real table sentinel must survive untouched"
    );
}

/// R1c-4 FINAL follow-up 3, decision 2: the database lifecycle guards
/// refuse RENAME/DROP (cascade — the sentinel child proves the refusal
/// happened BEFORE mutation) while a LIVE fence exists for the target —
/// both at the pre-CAS reservation and at the held gate — and admit both
/// operations once the run releases the gate (`mark_succeeded` + RAII
/// drop, no durable history needed): the rename preserves the target's
/// stable identity and sentinel, the cascade drop then removes the target.
#[test]
fn rename_and_drop_are_refused_under_a_live_gate_then_admitted_after_release() {
    on_big_stack(|| async {
        let guard = FlakeGuard::with_watchdog(
            "rename_and_drop_are_refused_under_a_live_gate_then_admitted_after_release",
            Duration::from_secs(150),
        );
        let scenario = Box::pin(async {
            // The TempDir stays alive for the whole test (no reopen here).
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

            // A REAL sentinel table in ALPHA: only a CASCADE drop destroys
            // it, so its survival proves the refusal happened before any
            // mutation.
            let mut sentinel_batch = Batch::new();
            sentinel_batch.op(
                "ct",
                ddl::create_table("sentinel_items").repo("main").build(),
            );
            let request = sentinel_batch.to_request_via_msgpack().unwrap();
            db.execute_as(OWNER, "alpha", &request)
                .await
                .expect("the sentinel create-table runs before the fence exists");

            // Reserve the in-process gate for ALPHA (pre-CAS
            // `GATE_PENDING`): in-memory only, no durable history.
            let mut fence = db
                .reserve_schema_migration_gate("alpha")
                .expect("the alpha gate is free for the reservation");

            // Under the RESERVATION: rename and cascade drop from the
            // HEALTHY session B are refused typed, target untouched.
            let rename_error =
                run_admin_op(&db, "beta", "rn", ddl::rename_db("alpha", "alpha2").build())
                    .await
                    .expect_err("rename under a reserved gate must be refused");
            assert_in_doubt(rename_error, "rename under a reserved gate");
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
            .expect_err("cascade drop under a reserved gate must be refused");
            assert_in_doubt(drop_error, "cascade drop under a reserved gate");
            assert!(db.has_db("alpha"), "alpha must survive the refused drop");
            assert_alpha_target_intact(&db, &alpha, "alpha", "under a reserved gate").await;

            // HOLD the gate (live runner): the same two operations stay
            // refused typed, target untouched.
            fence.hold(7);
            assert_eq!(
                db.schema_migration_gate_generation("alpha"),
                Some(7),
                "the held gate is visible to the fence map"
            );
            let rename_error =
                run_admin_op(&db, "beta", "rn", ddl::rename_db("alpha", "alpha2").build())
                    .await
                    .expect_err("rename under a held gate must be refused");
            assert_in_doubt(rename_error, "rename under a held gate");
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
            .expect_err("cascade drop under a held gate must be refused");
            assert_in_doubt(drop_error, "cascade drop under a held gate");
            assert_alpha_target_intact(&db, &alpha, "alpha", "under a held gate").await;

            // Release: the succeeded run drops the RAII without leaving an
            // in-doubt marker, so no false in-doubt state remains.
            fence.mark_succeeded();
            drop(fence);
            assert!(
                !db.schema_migration_in_doubt("alpha"),
                "a succeeded release must not seed an in-doubt marker"
            );

            // ADMITTED rename alpha -> alpha2 via B: the stable identity
            // and the sentinel travel with the re-keyed instance.
            let renamed =
                run_admin_op(&db, "beta", "rn", ddl::rename_db("alpha", "alpha2").build())
                    .await
                    .expect("the rename is admitted once the gate is released");
            assert_eq!(str_field(&renamed, "renamed_db"), "alpha");
            assert!(!db.has_db("alpha"), "the old name is gone after the rename");
            assert_alpha_target_intact(&db, &alpha, "alpha2", "after the admitted rename").await;

            // ADMITTED cascade drop of alpha2 via B: no target remains.
            run_admin_op(
                &db,
                "beta",
                "dd",
                ddl::drop_db("alpha2")
                    .hmac("test-hmac")
                    .cascade()
                    .build()
                    .expect("typed drop-db operation"),
            )
            .await
            .expect("the cascade drop is admitted once the gate is released");
            assert!(!db.has_db("alpha2"), "no target remains after the drop");
        });
        tokio::time::timeout(Duration::from_secs(300), scenario)
            .await
            .expect("live-gate lifecycle scenario exceeded its 300 s bound");
        guard.mark("scenario done");
    });
}

#[test]
fn rename_and_drop_of_an_in_doubt_database_are_refused_and_identity_and_journal_survive() {
    on_big_stack(|| async {
        let guard = FlakeGuard::with_watchdog(
            "rename_and_drop_of_an_in_doubt_database_are_refused_and_identity_and_journal_survive",
            Duration::from_secs(150),
        );
        let scenario = Box::pin(async {
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

            // A REAL table sentinel in ALPHA, before any fence: a CASCADE
            // drop would destroy it, so its survival is the observable that
            // the refusal happened before any mutation.
            let mut sentinel_batch = Batch::new();
            sentinel_batch.op(
                "ct",
                ddl::create_table("sentinel_items").repo("main").build(),
            );
            let request = sentinel_batch.to_request_via_msgpack().unwrap();
            db.execute_as(OWNER, "alpha", &request)
                .await
                .expect("the sentinel create-table runs before the fence is held");

            // ALPHA: durable OWNED fence with one Started event — the
            // in-flight state a crashed runner leaves behind. The migration
            // itself never runs, so the journal stays unresolved.
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

            // BETA: one fully applied migration so its post-restart
            // status/plan/apply assertions have a prefix to check.
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

            // Durable flush, drop, REOPEN: ALPHA comes back with the
            // open-time in-doubt marker seeded from its owned journal.
            let table = journal_table(&db).await;
            let db = flush_and_reopen(dir.path(), db, table).await;

            // STATUS alone first: ALPHA reports its in-doubt in-flight state.
            // A Started-only journal maps `in_flight.status` to "started"
            // (`event_kind_str`); the TOP-LEVEL error carries the typed
            // in-doubt code.
            let status = run_status(&db, "alpha", &alpha)
                .await
                .expect("alpha status reads its owned journal after the restart");
            let in_flight = field(&status, "in_flight");
            assert_eq!(str_field(in_flight, "migration_id"), "alpha-001");
            assert_eq!(field(in_flight, "fence_generation").as_i64(), Some(1));
            assert_eq!(str_field(in_flight, "status"), "started");
            assert_eq!(
                str_field(&status, "error"),
                "schema_migration_in_doubt",
                "alpha reports the typed in-doubt state on status"
            );

            // RENAME served from the HEALTHY session database B: the
            // session-scoped fence cannot see it — the target-side guard
            // must refuse before any catalogue re-key. Root principal
            // (`db.execute`), as every database-level admin op requires.
            let rename_error =
                run_admin_op(&db, "beta", "rn", ddl::rename_db("alpha", "alpha2").build())
                    .await
                    .expect_err("rename of an in-doubt database must be refused");
            assert_in_doubt(rename_error, "rename via healthy serving B");
            assert!(!db.has_db("alpha2"), "no shadow rename target may appear");
            assert_alpha_intact(&db, &alpha, "after the refused rename via B").await;

            // RENAME served from ALPHA itself: the batch-entry maintenance
            // fence refuses the whole batch with the same stable code.
            let rename_error_self = run_admin_op(
                &db,
                "alpha",
                "rn",
                ddl::rename_db("alpha", "alpha3").build(),
            )
            .await
            .expect_err("rename from the in-doubt session must be refused");
            assert_in_doubt(rename_error_self, "rename via A");
            assert_alpha_intact(&db, &alpha, "after the refused rename via A").await;

            // DROP served from the healthy session database B, WITH cascade:
            // refused before the CASCADE child teardown, so the sentinel and
            // the journal survive.
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
            .expect_err("cascade drop of an in-doubt database must be refused");
            assert_in_doubt(drop_error, "cascade drop via healthy serving B");
            assert!(db.has_db("alpha"), "alpha must survive the refused drop");
            assert_alpha_intact(&db, &alpha, "after the refused cascade drop").await;

            // Plain DROP from ALPHA's own session: refused the same way.
            let drop_error_self = run_admin_op(
                &db,
                "alpha",
                "dd",
                ddl::drop_db("alpha")
                    .hmac("test-hmac")
                    .build()
                    .expect("typed drop-db operation"),
            )
            .await
            .expect_err("drop from the in-doubt session must be refused");
            assert_in_doubt(drop_error_self, "drop via A");
            assert_alpha_intact(&db, &alpha, "after the refused drop via A").await;

            // CREATE DATABASE alpha IF NOT EXISTS: a no-op for the existing
            // name — and it must NOT replace ALPHA's stable identity or
            // clear the in-doubt state.
            let created = run_admin_op(
                &db,
                "beta",
                "cd",
                ddl::create_db("alpha").if_not_exists().build(),
            )
            .await
            .expect("create-if-not-exists for the existing name is a no-op");
            assert_eq!(
                field(&created, "created").as_bool(),
                Some(false),
                "nothing was created for the existing name"
            );
            assert_eq!(field(&created, "existed").as_bool(), Some(true));
            assert_alpha_intact(&db, &alpha, "after the no-op create-if-not-exists").await;

            // Plain CREATE DATABASE alpha: returns the typed `exists` error
            // (allowed), and certainly replaces nothing.
            let create_error = run_admin_op(&db, "beta", "cd", ddl::create_db("alpha").build())
                .await
                .expect_err("a plain create for the existing name must not replace it");
            assert_eq!(
                create_error.code(),
                Some("exists"),
                "plain create returns the exists refusal; message: {create_error}"
            );
            assert_alpha_intact(&db, &alpha, "after the refused plain create").await;

            // The healthy database B went through all of it unharmed.
            assert_beta_stays_healthy(&db, &beta, &beta_first).await;
        });
        tokio::time::timeout(Duration::from_secs(300), scenario)
            .await
            .expect("lifecycle refusal scenario exceeded its 300 s bound");
        guard.mark("scenario done");
    });
}

#[test]
fn resume_races_same_target_rename_and_cascade_drop() {
    on_big_stack(|| async {
        let guard = FlakeGuard::with_watchdog(
            "resume_races_same_target_rename_and_cascade_drop",
            Duration::from_secs(150),
        );
        let scenario = Box::pin(async {
            for (alias, op) in [
                ("rename", ddl::rename_db("alpha", "alpha2").build()),
                (
                    "drop",
                    ddl::drop_db("alpha")
                        .hmac("test-hmac")
                        .cascade()
                        .build()
                        .unwrap(),
                ),
            ] {
                let (dir, db) = persistent_db().await;
                db.create_db("alpha").await.unwrap();
                db.create_db("beta").await.unwrap();
                db.add_repo(
                    "alpha",
                    RepoConfig::new("main", BoxRepoFactory::fjall(dir.path().join("alpha-main"))),
                )
                .await
                .unwrap();
                // Lifecycle requires a root principal; use the same principal for
                // the migration so this fixture needs no synthetic grants.
                let alpha = db.database_identity_for_fence("alpha").unwrap();
                run_admin_op(
                    &db,
                    "alpha",
                    "sentinel",
                    ddl::create_table("sentinel_items").repo("main").build(),
                )
                .await
                .unwrap();
                let migration = body("alpha-001", 1);
                let resume = |generation| {
                    run_admin_op(
                        &db,
                        "alpha",
                        "apply",
                        shamir_query_builder::apply_application_schema_migration(
                            SchemaMigrationApplyRequest {
                                protocol_version: 1,
                                target: target(&alpha),
                                expected_predecessor: predecessor(None, 0, [0; 32]),
                                expected_fence_generation: generation,
                                migration: submission(&migration),
                            },
                        ),
                    )
                };

                db.system_store().arm_durability_tail_faults_after(2, 1);
                let error = Box::pin(resume(0)).await.unwrap_err();
                assert_eq!(
                    error.code(),
                    Some("schema_migration_durability_unavailable")
                );
                assert!(db.schema_migration_in_doubt("alpha"));
                guard.mark("Started persisted; racing recovery and lifecycle");

                // Fail the first intent again, this time after RecoveryAcquired.
                // There is no legal free interval anywhere in the race, even if
                // lifecycle runs before recovery begins or after it returns.
                db.system_store().arm_durability_tail_faults_after(2, 1);
                let start = tokio::sync::Barrier::new(2);
                let (recovery, lifecycle) = tokio::join!(
                    async {
                        start.wait().await;
                        Box::pin(resume(1)).await
                    },
                    async {
                        start.wait().await;
                        Box::pin(run_admin_op(&db, "beta", alias, op.clone())).await
                    },
                );
                let error = recovery.expect_err("the recovery intent fault must fire");
                assert_eq!(
                    error.code(),
                    Some("schema_migration_durability_unavailable")
                );
                assert_in_doubt(lifecycle.expect_err("there was no durable release"), alias);
                assert_alpha_target_intact(&db, &alpha, "alpha", alias).await;
                assert!(!db.has_db("alpha2"));
                assert!(db.schema_migration_in_doubt("alpha"));
                let history = db
                    .system_store()
                    .read_schema_migration_history(alpha)
                    .await
                    .unwrap();
                assert_eq!(
                    history
                        .events
                        .iter()
                        .map(|event| event.kind)
                        .collect::<Vec<_>>(),
                    [
                        JournalEventKind::Started,
                        JournalEventKind::RecoveryAcquired
                    ],
                );
                assert_eq!(history.unresolved.len(), 1);

                let applied = Box::pin(resume(2)).await.unwrap();
                assert_eq!(str_field(&applied, "status"), "applied");
                let history = db
                    .system_store()
                    .read_schema_migration_history(alpha)
                    .await
                    .unwrap();
                assert_eq!(history.applied.len(), 1);
                assert!(history.unresolved.is_empty());
                assert_eq!(
                    history.events.last().unwrap().kind,
                    JournalEventKind::FenceReleased,
                );
                assert!(!db.schema_migration_in_doubt("alpha"));
                Box::pin(run_admin_op(&db, "beta", alias, op))
                    .await
                    .expect("lifecycle is admitted after the verified durable release");
                assert!(!db.has_db("alpha"));
                if alias == "rename" {
                    assert_alpha_target_intact(&db, &alpha, "alpha2", alias).await;
                }
            }
        });
        tokio::time::timeout(Duration::from_secs(300), scenario)
            .await
            .expect("resume versus lifecycle race timed out");
        guard.mark("both lifecycle operations checked");
    });
}

#[test]
fn migration_and_lifecycle_cannot_both_reserve_the_same_identity() {
    on_big_stack(|| async {
        let guard = FlakeGuard::with_watchdog(
            "migration_and_lifecycle_cannot_both_reserve_the_same_identity",
            Duration::from_secs(150),
        );
        let scenario = Box::pin(async {
            let (_dir, db) = persistent_db().await;
            db.create_db("alpha").await.unwrap();
            let id = db.database_identity_for_fence("alpha").unwrap();

            // Compete on distinct worker threads. Keep both results alive until
            // both CAS attempts finish so a sequential release cannot give two wins.
            let start = std::sync::Arc::new(tokio::sync::Barrier::new(2));
            let attempted = std::sync::Arc::new(tokio::sync::Barrier::new(2));
            let contender = |migration: bool| {
                let db = db.clone();
                let start = start.clone();
                let attempted = attempted.clone();
                let runtime = tokio::runtime::Handle::current();
                tokio::task::spawn_blocking(move || {
                    runtime.block_on(async {
                        tokio::time::timeout(Duration::from_secs(120), start.wait())
                            .await
                            .expect("both contenders reached the start barrier");
                        let migration_gate = migration
                            .then(|| db.reserve_schema_migration_gate("alpha"))
                            .flatten();
                        let lifecycle_gate = (!migration)
                            .then(|| db.reserve_schema_migration_lifecycle(id))
                            .flatten();
                        tokio::time::timeout(Duration::from_secs(120), attempted.wait())
                            .await
                            .expect("both contenders attempted their CAS");
                        migration_gate.is_some() || lifecycle_gate.is_some()
                    })
                })
            };
            let (migration, lifecycle) = tokio::join!(contender(true), contender(false));
            assert_ne!(
                migration.unwrap(),
                lifecycle.unwrap(),
                "exactly one CAS wins",
            );

            // Exercise both directions regardless of which contender won.
            let lifecycle = db.reserve_schema_migration_lifecycle(id).unwrap();
            assert!(db.reserve_schema_migration_gate("alpha").is_none());
            drop(lifecycle);
            let mut migration = db.reserve_schema_migration_gate("alpha").unwrap();
            assert!(db.reserve_schema_migration_lifecycle(id).is_none());
            migration.hold(1);
            assert!(db.reserve_schema_migration_lifecycle(id).is_none());
            migration.mark_succeeded();
            drop(migration);
            assert!(db.reserve_schema_migration_lifecycle(id).is_some());
            assert!(!db.schema_migration_in_doubt("alpha"));
            let history = db
                .system_store()
                .read_schema_migration_history(id)
                .await
                .unwrap();
            assert!(
                history.events.is_empty(),
                "in-process CAS cannot write journal rows",
            );
        });
        tokio::time::timeout(Duration::from_secs(300), scenario)
            .await
            .expect("reservation race timed out");
        guard.mark("exclusive admission and RAII cleanup verified");
    });
}
