//! Phase 2b AX — admin-op authorization + owner-delegation (e2e).
//!
//! Two concerns proven against the real `execute_as` → `authorize_access`
//! path (no flags, no bypass except `Actor::System`):
//!
//!   A. **Closed hole.** A non-privileged `Actor::User` can no longer
//!      create users / groups / grant roles. `Actor::System` still can.
//!   B. **Owner delegation.** The owner of a database (holder of `Manage`
//!      on `ResourcePath::Database`) may create / drop users scoped to
//!      *their* database — without being a global admin — but not users
//!      scoped elsewhere, unscoped users, or users belonging to another db.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::auth::CreateUserOp;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

/// Build a ShamirDb with database "testdb"/repo "main"/table "users".
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Build a single-op batch request keyed by alias "op".
fn one_op(op: impl shamir_query_builder::batch::IntoBatchOp) -> BatchRequest {
    let mut b = Batch::new();
    b.id(1);
    b.op("op", op);
    b.to_request_via_msgpack()
}

// ===========================================================================
// Serde — CreateUserOp round-trips the optional `database` scope
// ===========================================================================

#[test]
fn create_user_op_serde_round_trip_with_database() {
    let op = CreateUserOp {
        create_user: "bob".to_string(),
        password: "pw".to_string().into(),
        roles: vec!["readonly".to_string()],
        profile: None,
        database: Some("testdb".to_string()),
    };
    assert_eq!(op.create_user, "bob");
    assert_eq!(op.database.as_deref(), Some("testdb"));

    // Serialize via msgpack → deserialize must preserve the scope.
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let op2: CreateUserOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(op2.database, op.database);
}

#[test]
fn create_user_op_serde_omits_database_when_absent() {
    let op = CreateUserOp {
        create_user: "bob".to_string(),
        password: "pw".to_string().into(),
        roles: vec![],
        profile: None,
        database: None,
    };
    assert_eq!(op.database, None);

    // skip_serializing_if = Option::is_none → no key when unset.
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let op2: CreateUserOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(op2.database, None);
}

// ===========================================================================
// A. Closed hole — non-privileged users cannot run user/role/group admin ops
// ===========================================================================

#[tokio::test]
async fn non_admin_cannot_create_user() {
    let shamir = setup().await;

    // Database is open (0o777) so `execute_as` Read-gate on the db passes,
    // proving the denial comes from the user-lifecycle check, not traversal.
    let req = one_op(ddl::create_user("mallory", "pw"));
    let resp = shamir.execute_as(Actor::User(7), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "non-admin user must NOT be able to create a user: {resp:?}"
    );
}

#[tokio::test]
async fn non_admin_cannot_create_group() {
    let shamir = setup().await;

    let req = one_op(ddl::create_group("devs"));
    let resp = shamir.execute_as(Actor::User(7), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "non-admin user must NOT be able to create a group: {resp:?}"
    );
}

#[tokio::test]
async fn non_admin_cannot_grant_role() {
    let shamir = setup().await;

    // Seed a user + role as System so the grant has real targets.
    let seed = one_op(ddl::create_user("alice", "pw"));
    shamir.execute("testdb", &seed).await.unwrap();
    let seed_role = one_op(ddl::create_role("analyst", vec![]));
    shamir.execute("testdb", &seed_role).await.unwrap();

    let req = one_op(ddl::grant_role("analyst", "alice"));
    let resp = shamir.execute_as(Actor::User(7), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "non-admin user must NOT be able to grant a role: {resp:?}"
    );
}

#[tokio::test]
async fn system_can_create_user_role_group() {
    let shamir = setup().await;

    // System (admin bypass) still drives every admin op.
    let cu = one_op(ddl::create_user("alice", "pw"));
    assert!(shamir.execute("testdb", &cu).await.is_ok());

    let cr = one_op(ddl::create_role("analyst", vec![]));
    assert!(shamir.execute("testdb", &cr).await.is_ok());

    let cg = one_op(ddl::create_group("devs"));
    assert!(shamir.execute("testdb", &cg).await.is_ok());
}

// ===========================================================================
// B. Owner delegation — the database owner manages users scoped to their db
// ===========================================================================

/// Make `actor` the owner of "testdb" (mode stays open so Read/Manage hold).
async fn make_db_owner(shamir: &ShamirDb, actor: Actor) {
    shamir
        .set_resource_meta(
            &ResourcePath::database("testdb"),
            &ResourceMeta {
                owner: actor,
                group: None,
                mode: 0o777,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn db_owner_can_create_scoped_user() {
    let shamir = setup().await;
    let owner = Actor::User(1001);
    make_db_owner(&shamir, owner.clone()).await;

    // Owner of "testdb" creates a user scoped to "testdb" → allowed.
    let req = one_op(ddl::create_user("scoped_bob", "pw").database("testdb"));
    let resp = shamir.execute_as(owner.clone(), "testdb", &req).await;
    assert!(
        resp.is_ok(),
        "db owner should create a user scoped to their db: {resp:?}"
    );
    assert_eq!(
        resp.unwrap().results["op"].records[0].get_value_str("created_user"),
        Some("scoped_bob")
    );
}

#[tokio::test]
async fn db_owner_cannot_create_unscoped_user() {
    let shamir = setup().await;
    let owner = Actor::User(1001);
    make_db_owner(&shamir, owner.clone()).await;

    // No `database` scope → only a global admin may create → denied.
    let req = one_op(ddl::create_user("global_user", "pw"));
    let resp = shamir.execute_as(owner.clone(), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "db owner must NOT create an unscoped (global) user: {resp:?}"
    );
}

#[tokio::test]
async fn db_owner_cannot_create_user_scoped_to_other_db() {
    let shamir = setup().await;
    let owner = Actor::User(1001);
    make_db_owner(&shamir, owner.clone()).await;

    // Scope points at a database the actor does not own → denied.
    let req = one_op(ddl::create_user("intruder", "pw").database("otherdb"));
    let resp = shamir.execute_as(owner.clone(), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "db owner must NOT create a user scoped to a foreign db: {resp:?}"
    );
}

#[tokio::test]
async fn db_owner_can_drop_own_scoped_user_but_not_foreign() {
    let shamir = setup().await;
    let owner = Actor::User(1001);
    make_db_owner(&shamir, owner.clone()).await;

    // Owner creates a scoped user for their db.
    let mk = one_op(ddl::create_user("scoped_bob", "pw").database("testdb"));
    shamir
        .execute_as(owner.clone(), "testdb", &mk)
        .await
        .unwrap();

    // System seeds a foreign-scoped user (owner does not own "otherdb").
    let mk_foreign = one_op(ddl::create_user("foreign_carol", "pw").database("otherdb"));
    shamir.execute("testdb", &mk_foreign).await.unwrap();

    // Owner drops THEIR scoped user → allowed.
    let drop_own = one_op(ddl::drop_user("scoped_bob"));
    let resp = shamir.execute_as(owner.clone(), "testdb", &drop_own).await;
    assert!(
        resp.is_ok(),
        "db owner should drop a user scoped to their db: {resp:?}"
    );

    // Owner tries to drop the foreign-scoped user → denied.
    let drop_foreign = one_op(ddl::drop_user("foreign_carol"));
    let resp = shamir
        .execute_as(owner.clone(), "testdb", &drop_foreign)
        .await;
    assert!(
        resp.is_err(),
        "db owner must NOT drop a user scoped to a foreign db: {resp:?}"
    );
}

#[tokio::test]
async fn scoped_user_is_persisted_with_database_field() {
    let shamir = setup().await;

    // System creates a scoped user; the scope must persist on the record so
    // the drop-path can resolve it.
    let mk = one_op(ddl::create_user("scoped_bob", "pw").database("testdb"));
    shamir.execute("testdb", &mk).await.unwrap();

    // Read the raw persisted user records from the system store and confirm
    // the `database` scope survived the round-trip onto disk.
    let users = shamir.system_store().load_users().await.unwrap();
    let rec = users
        .iter()
        .find(|u| u.get("name").and_then(|v| v.as_str()) == Some("scoped_bob"))
        .expect("scoped_bob must be persisted");
    assert_eq!(rec["database"], "testdb");
}

// ===========================================================================
// Authorization gate is non-vacuous: db owner gains/loses delegation with
// ownership.
// ===========================================================================

#[tokio::test]
async fn non_owner_with_db_read_still_cannot_create_scoped_user() {
    let shamir = setup().await;
    // User(1001) owns the db; User(2002) does NOT but the db is open so it
    // can Read/enter execute_as. It must still be denied the scoped create.
    make_db_owner(&shamir, Actor::User(1001)).await;

    let req = one_op(ddl::create_user("x", "pw").database("testdb"));
    let resp = shamir.execute_as(Actor::User(2002), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "a non-owner must NOT create a scoped user even with db read: {resp:?}"
    );

    // Sanity: the actual owner can.
    let resp = shamir.execute_as(Actor::User(1001), "testdb", &req).await;
    assert!(resp.is_ok(), "owner path must work: {resp:?}");
}
