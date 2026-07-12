//! Integration tests for access-control DDL (S3):
//! chmod / chown / chgrp + group CRUD → enforcement.
//!
//! Drives ops through `ShamirDb::execute` with admin batches, then
//! verifies that `authorize_access` actually enforces the changed
//! metadata — the DDL is non-vacuous.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::access::{Action, Actor, ResourceMeta, ResourcePath};

/// Helper: create a ShamirDb with database "testdb", repo "main", table "users".
///
/// G.4c: new objects default to enforced (owner-rwx 0o700). These tests
/// exercise chmod/chown/chgrp on the TABLE, so the db + store ancestors are
/// opened here to keep traversal-Execute from masking the table-level
/// behaviour under test.
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", repo_config).await.unwrap();
    let open = ResourceMeta::open();
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "main"), &open)
        .await
        .unwrap();
    shamir
}

/// Execute a single-op admin batch against "testdb".
async fn exec_op(
    shamir: &ShamirDb,
    op: impl shamir_query_builder::batch::IntoBatchOp,
) -> shamir_db::query::read::QueryResult {
    let mut b = Batch::new();
    b.id(1);
    b.op("op", op);
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    resp.results["op"].clone()
}

// ============================================================================
// chmod_then_enforced: DDL changes mode → enforcement denies non-owner
// ============================================================================

#[tokio::test]
async fn chmod_then_enforced() {
    let shamir = setup().await;
    let table_path = ResourcePath::table("testdb", "main", "users");

    // Step 1: chown the table to User(7) via DDL
    let result = exec_op(
        &shamir,
        ddl::chown(ddl::res::table("testdb", "main", "users"), 7),
    )
    .await;
    assert_eq!(result.records[0].get_value_i64("owner"), Some(7));

    // Step 2: chmod the table to 0o700 (owner rwx only) via DDL
    let result = exec_op(
        &shamir,
        ddl::chmod(ddl::res::table("testdb", "main", "users"), 0o700),
    )
    .await;
    assert_eq!(result.records[0].get_value_i64("mode"), Some(448));

    // Verify the meta was actually written
    let meta = shamir.resource_meta(&table_path).await.unwrap();
    assert_eq!(meta.owner, Actor::User(7));
    assert_eq!(meta.mode, 0o700);

    // Owner User(7) can read
    assert!(
        shamir
            .authorize_access(&Actor::User(7), &table_path, Action::Read)
            .await
            .is_ok(),
        "owner should be able to read after chmod 0o700"
    );

    // Non-owner User(8) is denied
    assert!(
        shamir
            .authorize_access(&Actor::User(8), &table_path, Action::Read)
            .await
            .is_err(),
        "non-owner should be denied after chmod 0o700"
    );
}

// ============================================================================
// group_grant_via_ddl: create_group + add_group_member + chgrp + chmod
// → group member allowed, non-member denied
// ============================================================================

#[tokio::test]
async fn group_grant_via_ddl() {
    let shamir = setup().await;
    let table_path = ResourcePath::table("testdb", "main", "users");

    // Step 1: chown table to User(10)
    exec_op(
        &shamir,
        ddl::chown(ddl::res::table("testdb", "main", "users"), 10),
    )
    .await;

    // Step 2: create group "devs"
    let result = exec_op(&shamir, ddl::create_group("devs")).await;
    let group_id = result.records[0].get_value_u64("group_id").unwrap();

    // Step 3: add User(8) to the group
    exec_op(
        &shamir,
        ddl::add_group_member(
            ddl::GroupRef::Name {
                name: "devs".into(),
            },
            8,
        ),
    )
    .await;

    // Step 4: chgrp the table to that group
    exec_op(
        &shamir,
        ddl::chgrp(ddl::res::table("testdb", "main", "users"), Some(group_id)),
    )
    .await;

    // Step 5: chmod to 0o750 (owner rwx, group r-x, other ---)
    exec_op(
        &shamir,
        ddl::chmod(ddl::res::table("testdb", "main", "users"), 0o750),
    )
    .await;

    // Verify meta
    let meta = shamir.resource_meta(&table_path).await.unwrap();
    assert_eq!(meta.owner, Actor::User(10));
    assert_eq!(meta.group, Some(group_id));
    assert_eq!(meta.mode, 0o750);

    // User(8) is in the group → can read (group has r)
    assert!(
        shamir
            .authorize_access(&Actor::User(8), &table_path, Action::Read)
            .await
            .is_ok(),
        "group member should be able to read"
    );

    // User(8) is in the group → can execute (group has x)
    assert!(
        shamir
            .authorize_access(&Actor::User(8), &table_path, Action::Execute)
            .await
            .is_ok(),
        "group member should be able to execute"
    );

    // User(8) in the group → cannot write (group has no w bit in 0o750)
    assert!(
        shamir
            .authorize_access(&Actor::User(8), &table_path, Action::Write)
            .await
            .is_err(),
        "group member should be denied write when group lacks w"
    );

    // User(9) is NOT in the group → denied (other bits are 0)
    assert!(
        shamir
            .authorize_access(&Actor::User(9), &table_path, Action::Read)
            .await
            .is_err(),
        "non-member should be denied read"
    );
}

// ============================================================================
// chown_changes_owner: chown via DDL → resource_meta reflects new owner
// ============================================================================

#[tokio::test]
async fn chown_changes_owner() {
    let shamir = setup().await;
    let table_path = ResourcePath::table("testdb", "main", "users");

    // Default owner is System
    let meta = shamir.resource_meta(&table_path).await.unwrap();
    assert_eq!(meta.owner, Actor::System);

    // chown to User(42)
    exec_op(
        &shamir,
        ddl::chown(ddl::res::table("testdb", "main", "users"), 42),
    )
    .await;

    let meta = shamir.resource_meta(&table_path).await.unwrap();
    assert_eq!(meta.owner, Actor::User(42));

    // chown again to User(99)
    exec_op(
        &shamir,
        ddl::chown(ddl::res::table("testdb", "main", "users"), 99),
    )
    .await;

    let meta = shamir.resource_meta(&table_path).await.unwrap();
    assert_eq!(meta.owner, Actor::User(99));
}

// ============================================================================
// chgrp_clears_group: chgrp with group: null clears the group
// ============================================================================

#[tokio::test]
async fn chgrp_clears_group() {
    let shamir = setup().await;
    let table_path = ResourcePath::table("testdb", "main", "users");

    // Create a group and chgrp to it
    let result = exec_op(&shamir, ddl::create_group("testers")).await;
    let gid = result.records[0].get_value_u64("group_id").unwrap();

    exec_op(
        &shamir,
        ddl::chgrp(ddl::res::table("testdb", "main", "users"), Some(gid)),
    )
    .await;

    let meta = shamir.resource_meta(&table_path).await.unwrap();
    assert_eq!(meta.group, Some(gid));

    // Clear the group
    exec_op(
        &shamir,
        ddl::chgrp(ddl::res::table("testdb", "main", "users"), None),
    )
    .await;

    let meta = shamir.resource_meta(&table_path).await.unwrap();
    assert!(meta.group.is_none());
}

// ============================================================================
// drop_group_removes_members: drop_group removes group; previously-group
// member is now denied
// ============================================================================

#[tokio::test]
async fn drop_group_removes_access() {
    let shamir = setup().await;
    let table_path = ResourcePath::table("testdb", "main", "users");

    // chown to User(10)
    exec_op(
        &shamir,
        ddl::chown(ddl::res::table("testdb", "main", "users"), 10),
    )
    .await;

    // Create group, add User(20)
    let result = exec_op(&shamir, ddl::create_group("temp")).await;
    let gid = result.records[0].get_value_u64("group_id").unwrap();

    exec_op(
        &shamir,
        ddl::add_group_member(
            ddl::GroupRef::Name {
                name: "temp".into(),
            },
            20,
        ),
    )
    .await;

    // chgrp + chmod group-readable
    exec_op(
        &shamir,
        ddl::chgrp(ddl::res::table("testdb", "main", "users"), Some(gid)),
    )
    .await;

    exec_op(
        &shamir,
        ddl::chmod(ddl::res::table("testdb", "main", "users"), 0o770),
    )
    .await;

    // User(20) can read (in group, group has rwx)
    assert!(shamir
        .authorize_access(&Actor::User(20), &table_path, Action::Read)
        .await
        .is_ok());

    // Remove User(20) from group
    exec_op(
        &shamir,
        ddl::remove_group_member(
            ddl::GroupRef::Name {
                name: "temp".into(),
            },
            20,
        ),
    )
    .await;

    // User(20) is no longer in the group → denied (other bits are 0)
    assert!(
        shamir
            .authorize_access(&Actor::User(20), &table_path, Action::Read)
            .await
            .is_err(),
        "removed member should be denied"
    );
}

// ============================================================================
// chmod_on_database: chmod works on database-level resource
// ============================================================================

#[tokio::test]
async fn chmod_database_resource() {
    let shamir = setup().await;
    let db_path = ResourcePath::database("testdb");

    // Restrict the database to 0o700, owner User(1)
    exec_op(&shamir, ddl::chown(ddl::res::database("testdb"), 1)).await;

    exec_op(&shamir, ddl::chmod(ddl::res::database("testdb"), 0o700)).await;

    let meta = shamir.resource_meta(&db_path).await.unwrap();
    assert_eq!(meta.owner, Actor::User(1));
    assert_eq!(meta.mode, 0o700);

    // User(2) cannot traverse the database (no Execute bit for other)
    assert!(
        shamir
            .authorize_access(
                &Actor::User(2),
                &ResourcePath::table("testdb", "main", "users"),
                Action::Read,
            )
            .await
            .is_err(),
        "should be denied at database traversal"
    );
}

// ============================================================================
// group_owner_can_manage_own_group_via_wire_without_root_manage (task #552
// adversarial-review regression): the group-CRUD OR-gate
// (Manage(Root) OR Manage(Group{name})) must actually be reachable through
// the WIRE DISPATCHER, not just the internal `ShamirDb::*_as` methods. A
// review of an earlier revision found the dispatcher
// (`shamir_db::execute::admin_access`) still ran its own, older,
// unconditional `Manage(Root)`-only pre-check ahead of the corrected `*_as`
// methods, silently pre-rejecting every non-Root-Manage caller — making the
// whole feature unreachable from any real client even though the internal
// methods were themselves correct. This test drives the ops through
// `ShamirDb::execute_as` (the exact entry point the server uses), not the
// internal methods directly, so it would have caught that regression.
// ============================================================================

#[tokio::test]
async fn group_owner_can_manage_own_group_via_wire_without_root_manage() {
    let shamir = setup().await;
    let creator = Actor::User(500);
    let stranger = Actor::User(501);

    // Bootstrap: grant `creator` Manage(Root) just long enough to create the
    // group (creation itself is still gated on Manage(Root) only — see the
    // brief), then REVOKE it, proving every subsequent op works on
    // ownership alone, through the real wire path.
    shamir
        .set_resource_meta(
            &ResourcePath::Root,
            &ResourceMeta {
                owner: creator.clone(),
                group: None,
                mode: 0o755,
            },
        )
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(1);
    b.create_group("op", ddl::create_group("wire-devs"));
    let req = b.to_request_via_msgpack();
    let resp = shamir
        .execute_as(creator.clone(), "testdb", &req)
        .await
        .unwrap();
    let gid = resp.results["op"].records[0]
        .get_value_u64("group_id")
        .unwrap();

    // Revoke creator's Manage(Root) — hand Root back to System.
    shamir
        .set_resource_meta(
            &ResourcePath::Root,
            &ResourceMeta {
                owner: Actor::System,
                group: None,
                mode: 0o755,
            },
        )
        .await
        .unwrap();

    // Creator can still rename/add-member/remove-member/drop THEIR OWN
    // group through the wire dispatcher, without Manage(Root).
    let group_ref = ddl::GroupRef::Name {
        name: "wire-devs".into(),
    };
    let mut b = Batch::new();
    b.id(1);
    b.rename_group("op", ddl::rename_group(group_ref.clone(), "wire-devs-2"));
    let req = b.to_request_via_msgpack();
    shamir
        .execute_as(creator.clone(), "testdb", &req)
        .await
        .expect("group owner must be able to rename their own group via the wire dispatcher");

    let group_ref = ddl::GroupRef::Name {
        name: "wire-devs-2".into(),
    };
    let mut b = Batch::new();
    b.id(1);
    b.add_group_member("op", ddl::add_group_member(group_ref.clone(), 999));
    let req = b.to_request_via_msgpack();
    shamir
        .execute_as(creator.clone(), "testdb", &req)
        .await
        .expect(
            "group owner must be able to add members to their own group via the wire dispatcher",
        );

    let mut b = Batch::new();
    b.id(1);
    b.remove_group_member("op", ddl::remove_group_member(group_ref.clone(), 999));
    let req = b.to_request_via_msgpack();
    shamir
        .execute_as(creator.clone(), "testdb", &req)
        .await
        .expect(
        "group owner must be able to remove members from their own group via the wire dispatcher",
    );

    let mut b = Batch::new();
    b.id(1);
    b.drop_group("op", ddl::drop_group(ddl::GroupRef::Id { id: gid }));
    let req = b.to_request_via_msgpack();
    shamir
        .execute_as(creator, "testdb", &req)
        .await
        .expect("group owner must be able to drop their own group via the wire dispatcher");

    // A stranger — neither the owner nor holding Manage(Root) — is denied
    // the same ops via the wire dispatcher (recreate the group first, since
    // the creator just dropped theirs above).
    shamir
        .set_resource_meta(
            &ResourcePath::Root,
            &ResourceMeta {
                owner: Actor::System,
                group: None,
                mode: 0o777,
            },
        )
        .await
        .unwrap();
    let result = exec_op(&shamir, ddl::create_group("wire-devs-owned")).await;
    let gid2 = result.records[0].get_value_u64("group_id").unwrap();
    // Stamp a real owner (not the stranger) directly, simulating a group
    // created by someone else.
    shamir
        .set_resource_meta(
            &ResourcePath::group("wire-devs-owned"),
            &ResourceMeta {
                owner: Actor::User(600),
                group: None,
                mode: 0o750,
            },
        )
        .await
        .unwrap();

    let group_ref = ddl::GroupRef::Id { id: gid2 };
    let mut b = Batch::new();
    b.id(1);
    b.rename_group("op", ddl::rename_group(group_ref.clone(), "should-fail"));
    let req = b.to_request_via_msgpack();
    let err = shamir
        .execute_as(stranger.clone(), "testdb", &req)
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("access_denied"),
        "a non-owner, non-Manage(Root) actor must be denied rename_group via the wire dispatcher"
    );

    let mut b = Batch::new();
    b.id(1);
    b.drop_group("op", ddl::drop_group(group_ref));
    let req = b.to_request_via_msgpack();
    let err = shamir
        .execute_as(stranger, "testdb", &req)
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("access_denied"),
        "a non-owner, non-Manage(Root) actor must be denied drop_group via the wire dispatcher"
    );
}
