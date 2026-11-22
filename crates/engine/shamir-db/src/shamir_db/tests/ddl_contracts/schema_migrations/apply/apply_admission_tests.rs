//! Server-side tests for the ADMISSION half of application-schema APPLY
//! (R1c step 2b-3a): `Manage` authorization, durable-prefix refusals without
//! writes, per-DDL-target authorization before intent, and follower refusal.
//! Function-family pre-intent coverage lives in the sibling function_family suite.

use base64::Engine as _;
use std::future::Future;
use std::time::Duration;

use shamir_query_builder::SchemaMigration;
use shamir_query_builder::{batch::Batch, ddl};
use shamir_query_types::WriteOp;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

use super::super::helpers::body;
use super::helpers::{
    apply_request, assert_code, empty_predecessor, grant_other_read_only, identity, journal_rows,
    persistent_owner_db, run_apply, run_apply_as, str_field, submission, submission_with_checksum,
};
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use shamir_test_diag::FlakeGuard;

/// Review finding 3 (84-fence-round3): the foreign database that binds the
/// renamed validator.
const FOREIGN: &str = "foreign";
const FOREIGN_REPO: &str = "main";
const FOREIGN_TABLE: &str = "sentinel";
const VALIDATOR: &str = "shared_guard";
const RENAMED: &str = "shared_guard_moved";

/// The same minimal uploaded-module ABI as the validator target-fence
/// fixture; rename fingerprints the stored row, so the artifact only has to
/// compile and persist.
fn wasm(memory_pages: u32) -> String {
    base64::engine::general_purpose::STANDARD.encode(
        wat::parse_str(format!(
            r#"(module
        (memory (export "memory") {memory_pages})
        (func (export "shamir_alloc") (param i32) (result i32) i32.const 1024)
        (func (export "shamir_call") (param i32 i32) (result i64) i64.const 0)
    )"#
        ))
        .unwrap(),
    )
}

/// Indexed 180s timeout around the foreign-fence rename test body: on
/// timeout the test panics and the engine is not reused.
async fn bounded<T>(step: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(180), Box::pin(step))
        .await
        .expect("foreign-fence rename test body timed out")
}

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

/// A RenameValidator whose validator is bound in a FOREIGN database takes
/// that holder's target admission BEFORE any journal write (review finding
/// 3, 84-fence-round3): a foreign RESERVED fence is refused with the
/// foreign target's stable `schema_migration_in_progress` code naming the
/// holder, the serving journal stays pristine (the retry needs no recovery),
/// the validator keeps its id and durable name, and the serving database's
/// own fence stays obtainable. Once the foreign fence clears, the identical
/// request applies and the rename re-keys the bound id.
#[test]
fn a_rename_of_a_validator_bound_in_a_fenced_foreign_database_refuses_before_any_intent() {
    super::super::helpers::on_big_stack(|| bounded(foreign_fenced_validator_rename()));
}

async fn foreign_fenced_validator_rename() {
    let guard = FlakeGuard::with_watchdog(
        "a_rename_of_a_validator_bound_in_a_fenced_foreign_database_refuses_before_any_intent",
        Duration::from_secs(150),
    );
    let (_dir, db) = persistent_owner_db().await;
    guard.mark("serving database ready");
    let target = identity(&db).await;

    // The foreign binding holder: an in-memory repo/table is enough — the
    // durable system journal is shared with `testdb` and is what the
    // no-write assertion reads.
    db.create_db(FOREIGN).await.unwrap();
    db.add_repo(
        FOREIGN,
        RepoConfig::new(FOREIGN_REPO, BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    let mut table = Batch::new();
    table.op(
        "table",
        ddl::create_table(FOREIGN_TABLE).repo(FOREIGN_REPO).build(),
    );
    db.execute(FOREIGN, &table.to_request_via_msgpack().unwrap())
        .await
        .unwrap();

    // Server-global validator registration and the foreign-only binding,
    // both through the ordinary wire dispatch (root-principal setup; no
    // fence is held yet).
    super::super::helpers::run_admin_op(
        &db,
        "testdb",
        "validator",
        ddl::create_validator(VALIDATOR)
            .wasm(wasm(2))
            .build()
            .unwrap(),
    )
    .await
    .expect("the fixture validator is registered");
    super::super::helpers::run_admin_op(
        &db,
        "testdb",
        "bind",
        ddl::bind_validator(VALIDATOR, FOREIGN_TABLE)
            .db(FOREIGN)
            .repo(FOREIGN_REPO)
            .ops([WriteOp::Insert])
            .build(),
    )
    .await
    .expect("the fixture binding is created");
    let validator_id = db.validators().id_for_name(VALIDATOR).unwrap();
    let pristine = journal_rows(&db).await;
    guard.mark("validator registered, foreign-bound, journal snapshotted");

    // The foreign migration fence is reserved in this process.
    let gate = db
        .reserve_schema_migration_gate(FOREIGN)
        .expect("the foreign fence is vacant before the test reserves it");

    // The served migration body renames the foreign-bound validator.
    let migration = SchemaMigration::new("ren-001", 1, "renames a foreign-bound validator")
        .op(ddl::rename_validator(VALIDATOR, RENAMED))
        .expect("typed rename-validator operation is admitted")
        .build()
        .expect("test migration body is valid");
    let request = apply_request(&target, empty_predecessor(), 0, submission(&migration));

    let error = run_apply(&db, request.clone())
        .await
        .expect_err("a foreign fenced binding holder must stop the migration");
    let message = error.to_string();
    assert_code(error, "schema_migration_in_progress");
    assert!(
        message.contains(FOREIGN),
        "the refusal must name the fenced binding holder: {message}"
    );

    // Refused BEFORE the intent: the journal is untouched, the validator
    // keeps its id and name, and the serving database's own fence stays
    // obtainable (A is not stranded by the foreign refusal).
    assert_eq!(
        journal_rows(&db).await,
        pristine,
        "the foreign admission refusal must not write a journal event"
    );
    assert_eq!(db.validators().id_for_name(VALIDATOR), Some(validator_id));
    assert_eq!(db.validators().id_for_name(RENAMED), None);
    // The deep case must prove the clean SERVING gate is truly held here:
    // without `.expect` this is a no-op when the gate is stranded by the
    // foreign refusal, silently weakening the no-strand assertion.
    drop(
        db.reserve_schema_migration_gate("testdb")
            .expect("the serving database's own fence stays obtainable after the foreign refusal"),
    );
    guard.mark("serving fence re-reserved and released");

    // With the foreign fence dropped, the identical request executes.
    drop(gate);
    let value = run_apply(&db, request)
        .await
        .expect("the retry after the foreign fence clears must apply");
    assert_eq!(str_field(&value, "status"), "applied");
    assert_eq!(db.validators().id_for_name(VALIDATOR), None);
    assert_eq!(db.validators().id_for_name(RENAMED), Some(validator_id));
    assert!(
        journal_rows(&db).await.len() > pristine.len(),
        "the retry applied the migration"
    );
    guard.mark("retry applied with the foreign fence cleared");
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
