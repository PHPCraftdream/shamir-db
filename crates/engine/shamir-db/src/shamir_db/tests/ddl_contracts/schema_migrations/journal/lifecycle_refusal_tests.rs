//! Lifecycle refusals for a quarantined database (R1c-4 follow-up P1,
//! design §5.3): with a durable OWNED `Started` event and an open-time
//! in-doubt marker standing after a cold restart, database-level admin DDL
//! — rename via a healthy session database, rename via the quarantined
//! session itself, CASCADE drop, plain drop, and create (no-op and typed
//! `exists` refusal) — is refused with `schema_migration_in_doubt` before
//! any mutation, the stable identity and unresolved journal survive intact,
//! and a healthy neighbor keeps its full status/plan/actual-apply path.
//!
//! Database-level admin DDL authorizes at `ResourcePath::Root`, so every
//! admin op runs through the root principal (`db.execute` =
//! `Actor::System`); the ordinary owner principal (`User(7)`) has no root
//! grant. Application-schema applies run under the owner principal, as in
//! production.

use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::types::schema_migration::SchemaMigrationBody;
use shamir_test_diag::FlakeGuard;

use super::super::helpers::on_big_stack;
use super::super::helpers::{
    body, checksum, context, fence_generation_from_status, field, flush_and_reopen, journal_table,
    persistent_db, predecessor, prepare_database, run_admin_op, run_apply, run_plan, run_status,
    str_field, submission, OWNER,
};
use crate::query::batch::BatchError;
use crate::shamir_db::system_store::DatabaseIdentity;
use crate::ShamirDb;

fn assert_in_doubt(error: BatchError, context: &str) {
    assert_eq!(
        error.code(),
        Some("schema_migration_in_doubt"),
        "{context}: wrong refusal code; message: {error}"
    );
}

/// ALPHA is still exactly as the refusals left it: same stable identity,
/// unresolved owned journal intact under that identity, sentinel table
/// present, marker still refusing an ordinary write.
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
    // The quarantine still refuses an ordinary write on ALPHA: nothing in
    // the refused lifecycle operations cleared the marker.
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

#[test]
fn rename_and_drop_of_a_quarantined_database_are_refused_and_identity_and_journal_survive() {
    on_big_stack(|| async {
        let guard = FlakeGuard::with_watchdog(
            "rename_and_drop_of_a_quarantined_database_are_refused_and_identity_and_journal_survive",
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
            // open-time in-doubt marker seeded from the owned journal.
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
                    .expect_err("rename of a quarantined database must be refused");
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
            .expect_err("rename from the quarantined session must be refused");
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
            .expect_err("cascade drop of a quarantined database must be refused");
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
            .expect_err("drop from the quarantined session must be refused");
            assert_in_doubt(drop_error_self, "drop via A");
            assert_alpha_intact(&db, &alpha, "after the refused drop via A").await;

            // CREATE DATABASE alpha IF NOT EXISTS: a no-op for the existing
            // name — and it must NOT replace ALPHA's stable identity or
            // clear the quarantine.
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
