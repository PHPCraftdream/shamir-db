//! Integration tests for access-control DDL (S3):
//! chmod / chown / chgrp + group CRUD → enforcement.
//!
//! Drives ops through `ShamirDb::execute` with admin batches, then
//! verifies that `authorize_access` actually enforces the changed
//! metadata — the DDL is non-vacuous.

use serde_json::json;
use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_types::access::{Action, Actor, ResourcePath};

/// Helper: create a ShamirDb with database "testdb", repo "main", table "users".
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", repo_config).await.unwrap();
    shamir
}

/// Execute a single-op admin batch against "testdb".
async fn exec_op(
    shamir: &ShamirDb,
    op_json: serde_json::Value,
) -> shamir_db::query::read::QueryResult {
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "op": op_json
        }
    }))
    .unwrap();
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
        json!({
            "chown": {
                "table": ["testdb", "main", "users"]
            },
            "owner": 7
        }),
    )
    .await;
    assert_eq!(result.records[0]["owner"], 7);

    // Step 2: chmod the table to 0o700 (owner rwx only) via DDL
    let result = exec_op(
        &shamir,
        json!({
            "chmod": {
                "table": ["testdb", "main", "users"]
            },
            "mode": 448   // 0o700
        }),
    )
    .await;
    assert_eq!(result.records[0]["mode"], 448);

    // Verify the meta was actually written
    let meta = shamir.resource_meta(&table_path).await;
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
        json!({
            "chown": {
                "table": ["testdb", "main", "users"]
            },
            "owner": 10
        }),
    )
    .await;

    // Step 2: create group "devs"
    let result = exec_op(
        &shamir,
        json!({
            "create_group": "devs"
        }),
    )
    .await;
    let group_id = result.records[0]["group_id"].as_u64().unwrap();

    // Step 3: add User(8) to the group
    exec_op(
        &shamir,
        json!({
            "add_group_member": {
                "name": "devs"
            },
            "user": 8
        }),
    )
    .await;

    // Step 4: chgrp the table to that group
    exec_op(
        &shamir,
        json!({
            "chgrp": {
                "table": ["testdb", "main", "users"]
            },
            "group": group_id
        }),
    )
    .await;

    // Step 5: chmod to 0o750 (owner rwx, group r-x, other ---)
    exec_op(
        &shamir,
        json!({
            "chmod": {
                "table": ["testdb", "main", "users"]
            },
            "mode": 488   // 0o750
        }),
    )
    .await;

    // Verify meta
    let meta = shamir.resource_meta(&table_path).await;
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
    let meta = shamir.resource_meta(&table_path).await;
    assert_eq!(meta.owner, Actor::System);

    // chown to User(42)
    exec_op(
        &shamir,
        json!({
            "chown": {
                "table": ["testdb", "main", "users"]
            },
            "owner": 42
        }),
    )
    .await;

    let meta = shamir.resource_meta(&table_path).await;
    assert_eq!(meta.owner, Actor::User(42));

    // chown again to User(99)
    exec_op(
        &shamir,
        json!({
            "chown": {
                "table": ["testdb", "main", "users"]
            },
            "owner": 99
        }),
    )
    .await;

    let meta = shamir.resource_meta(&table_path).await;
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
    let result = exec_op(
        &shamir,
        json!({
            "create_group": "testers"
        }),
    )
    .await;
    let gid = result.records[0]["group_id"].as_u64().unwrap();

    exec_op(
        &shamir,
        json!({
            "chgrp": {
                "table": ["testdb", "main", "users"]
            },
            "group": gid
        }),
    )
    .await;

    let meta = shamir.resource_meta(&table_path).await;
    assert_eq!(meta.group, Some(gid));

    // Clear the group
    exec_op(
        &shamir,
        json!({
            "chgrp": {
                "table": ["testdb", "main", "users"]
            },
            "group": null
        }),
    )
    .await;

    let meta = shamir.resource_meta(&table_path).await;
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
        json!({
            "chown": {
                "table": ["testdb", "main", "users"]
            },
            "owner": 10
        }),
    )
    .await;

    // Create group, add User(20)
    let result = exec_op(
        &shamir,
        json!({
            "create_group": "temp"
        }),
    )
    .await;
    let gid = result.records[0]["group_id"].as_u64().unwrap();

    exec_op(
        &shamir,
        json!({
            "add_group_member": {
                "name": "temp"
            },
            "user": 20
        }),
    )
    .await;

    // chgrp + chmod group-readable
    exec_op(
        &shamir,
        json!({
            "chgrp": {
                "table": ["testdb", "main", "users"]
            },
            "group": gid
        }),
    )
    .await;

    exec_op(
        &shamir,
        json!({
            "chmod": {
                "table": ["testdb", "main", "users"]
            },
            "mode": 504   // 0o770
        }),
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
        json!({
            "remove_group_member": {
                "name": "temp"
            },
            "user": 20
        }),
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
    exec_op(
        &shamir,
        json!({
            "chown": {
                "database": "testdb"
            },
            "owner": 1
        }),
    )
    .await;

    exec_op(
        &shamir,
        json!({
            "chmod": {
                "database": "testdb"
            },
            "mode": 448   // 0o700
        }),
    )
    .await;

    let meta = shamir.resource_meta(&db_path).await;
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
