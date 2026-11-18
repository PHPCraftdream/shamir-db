//! Server-side tests for the ADMISSION half of application-schema APPLY
//! (R1c step 2b-3a): `Manage` authorization, durable-prefix refusals without
//! writes, per-DDL-target authorization before intent, and follower refusal.
//! Function-family pre-intent coverage lives in the sibling function_family suite.

use shamir_query_builder::ddl;
use shamir_query_builder::SchemaMigration;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

use super::super::helpers::body;
use super::helpers::{
    apply_request, assert_code, empty_predecessor, grant_other_read_only, identity, journal_rows,
    persistent_owner_db, run_apply, run_apply_as, str_field, submission, submission_with_checksum,
};

#[tokio::test]
async fn apply_refuses_a_read_only_caller_and_admits_the_owner() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = body("alpha-001", 1);

    // A NON-owner reaching the database through the "other" read bit holds
    // Read but not Manage (Manage is the owner/admin-only class). The
    // admitted half below runs as the owner, never the system actor.
    grant_other_read_only(&db).await;
    let error = run_apply_as(
        &db,
        super::helpers::OTHER,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect_err("a Read-only caller must be refused");
    assert_code(error, "access_denied");

    // The owner holds Manage and the migration executes.
    db.set_resource_meta(
        &ResourcePath::database("testdb"),
        &ResourceMeta {
            owner: super::helpers::OWNER,
            group: None,
            mode: 0o700,
        },
    )
    .await
    .unwrap();
    let value = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect("the owner holds Manage and apply is admitted");
    assert_eq!(str_field(&value, "status"), "applied");
}

#[tokio::test]
async fn wrong_predecessor_gap_checksum_mismatch_and_identity_mismatch_are_refused_without_writes()
{
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    db.create_db("other").await.unwrap();
    let foreign = db
        .system_store()
        .schema_migration_identity("other")
        .await
        .unwrap();
    let pristine = journal_rows(&db).await;

    // Wrong predecessor: the journal head is empty, not ghost/1.
    let error = run_apply(
        &db,
        apply_request(
            &target,
            super::helpers::predecessor(Some("ghost"), 1, [1; 32]),
            0,
            submission(&body("alpha-001", 1)),
        ),
    )
    .await
    .expect_err("a stale predecessor must be refused");
    assert_code(error, "schema_migration_predecessor_mismatch");

    // Ordinal gap: the journal continues at ordinal 1.
    let error = run_apply(
        &db,
        apply_request(
            &target,
            empty_predecessor(),
            0,
            submission(&body("alpha-003", 3)),
        ),
    )
    .await
    .expect_err("a gapped ordinal must be refused");
    assert_code(error, "schema_migration_gap");

    // Checksum mismatch: the submitted checksum disagrees with the body.
    let error = run_apply(
        &db,
        apply_request(
            &target,
            empty_predecessor(),
            0,
            submission_with_checksum(&body("alpha-001", 1), [9; 32]),
        ),
    )
    .await
    .expect_err("a wrong client checksum must be refused");
    assert_code(error, "schema_migration_checksum_mismatch");

    // Identity mismatch: the request targets another database's identity.
    let error = run_apply(
        &db,
        apply_request(
            &foreign,
            empty_predecessor(),
            0,
            submission(&body("alpha-001", 1)),
        ),
    )
    .await
    .expect_err("a foreign target must be refused");
    assert_code(error, "schema_migration_identity_mismatch");

    // Not one of the four refusals wrote a journal event.
    assert_eq!(
        journal_rows(&db).await,
        pristine,
        "admission refusals must write nothing"
    );
}

#[tokio::test]
async fn a_body_naming_a_missing_repository_is_refused_before_the_journal_is_touched() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;

    // The repository a body names is knowable at admission: the refusal must
    // happen BEFORE the durable `started` event would strand the journal in
    // the uncertain window.
    let migration = SchemaMigration::new("alpha-ghost", 1, "names a missing repository")
        .op(ddl::create_table("items_1").repo("ghost_repo").build())
        .expect("typed create-table operation is admitted")
        .build()
        .expect("test migration body is valid");

    let pristine = journal_rows(&db).await;
    let error = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect_err("a missing repository must be refused before any intent");
    let message = error.to_string();
    assert_code(error, "not_found");
    assert!(
        message.contains("alpha-ghost") && message.contains("ghost_repo"),
        "the refusal must name the migration and the missing repository: {message}"
    );
    assert_eq!(
        journal_rows(&db).await,
        pristine,
        "the preflight refusal must not write a started event"
    );
    assert!(db.schema_migration_gate_generation("testdb").is_none());
}

#[tokio::test]
async fn a_ddl_target_the_caller_may_not_touch_stops_the_migration_before_its_intent() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;

    // The fixture already grants [`OWNER`] the store resource (the preflight
    // authorizes CreateTable as `Create` on the store). This test DENIES that
    // specific resource again — the store is owned by another user with no
    // "other" bits — proving §5.2's per-target Create authorization stops the
    // migration even though the caller holds `Manage` on the database.
    db.set_resource_meta(
        &ResourcePath::store("testdb", "main"),
        &ResourceMeta {
            owner: Actor::User(99),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .unwrap();

    let pristine = journal_rows(&db).await;
    let error = run_apply(
        &db,
        apply_request(
            &target,
            empty_predecessor(),
            0,
            submission(&body("alpha-001", 1)),
        ),
    )
    .await
    .expect_err("an unauthorized DDL target must stop the migration");
    assert_code(error, "access_denied");

    // Refused BEFORE the intent: the journal is untouched and the table was
    // never created.
    assert_eq!(
        journal_rows(&db).await,
        pristine,
        "the preflight refusal must not write an intent"
    );
    assert!(!db.get_db("testdb").unwrap().has_table("main", "items_1"));
}

#[tokio::test]
async fn a_read_only_follower_node_refuses_apply() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = body("alpha-001", 1);
    let request = apply_request(&target, empty_predecessor(), 0, submission(&migration));

    // The server lowers its node mode once at launch; the engine fails
    // closed even for an otherwise fully authorized owner.
    db.set_node_read_only(true);
    let error = run_apply(&db, request.clone())
        .await
        .expect_err("a follower must refuse apply");
    assert_code(error, "schema_migration_follower_read_only");
    let pristine = journal_rows(&db).await;

    // Back on a primary, the same request executes.
    db.set_node_read_only(false);
    let value = run_apply(&db, request)
        .await
        .expect("a primary admits the same request");
    assert_eq!(str_field(&value, "status"), "applied");
    let after = journal_rows(&db).await;
    assert!(
        after.len() > pristine.len(),
        "the primary applied the migration"
    );
}
