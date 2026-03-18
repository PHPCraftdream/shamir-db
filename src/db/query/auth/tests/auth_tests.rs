//! End-to-end tests for auth operations via ShamirDb::execute.

use serde_json::json;

use crate::db::engine::repo::repo_types::BoxRepoFactory;
use crate::db::engine::repo::RepoConfig;
use crate::db::engine::table::TableConfig;
use crate::db::query::batch::BatchRequest;
use crate::db::ShamirDb;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

// ============================================================================
// Users CRUD
// ============================================================================

#[tokio::test]
async fn test_create_user() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "cu": {
                "create_user": "alice",
                "password": "secret123",
                "roles": ["readonly"],
                "profile": {"department": "engineering", "level": 3}
            }
        }
    })).unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["cu"].records[0]["created_user"], "alice");
}

#[tokio::test]
async fn test_list_users() {
    let shamir = setup_shamir().await;

    // Create two users
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "u1": {"create_user": "alice", "password": "pass1", "roles": ["readonly"]},
            "u2": {"create_user": "bob", "password": "pass2", "roles": ["readwrite"]}
        }
    })).unwrap();
    shamir.execute("testdb", &req).await.unwrap();

    // List users
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "list": {"list": "users"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();

    let users = resp.results["list"].records[0]["users"].as_array().unwrap();
    assert_eq!(users.len(), 2);

    // Password hash should NOT be in output
    for user in users {
        assert!(user.get("password_hash").is_none());
    }
}

#[tokio::test]
async fn test_drop_user() {
    let shamir = setup_shamir().await;

    // Create then drop
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "cu": {"create_user": "alice", "password": "pass", "roles": []}
        }
    })).unwrap();
    shamir.execute("testdb", &req).await.unwrap();

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "du": {"drop_user": "alice"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["du"].records[0]["existed"], true);

    // Drop non-existent
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 3,
        "queries": {
            "du": {"drop_user": "alice"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["du"].records[0]["existed"], false);
}

// ============================================================================
// Roles CRUD
// ============================================================================

#[tokio::test]
async fn test_create_role() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "cr": {
                "create_role": "analyst",
                "permissions": [
                    {
                        "effect": "allow",
                        "actions": ["read"],
                        "resource": {"scope": "global"}
                    },
                    {
                        "effect": "allow",
                        "actions": ["insert", "update"],
                        "resource": {
                            "scope": "table",
                            "database": "mydb",
                            "repo": "main",
                            "table": "reports"
                        }
                    }
                ]
            }
        }
    })).unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["cr"].records[0]["created_role"], "analyst");
}

#[tokio::test]
async fn test_list_roles() {
    let shamir = setup_shamir().await;

    // Create role
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "cr": {
                "create_role": "analyst",
                "permissions": [
                    {"effect": "allow", "actions": ["read"], "resource": {"scope": "global"}}
                ]
            }
        }
    })).unwrap();
    shamir.execute("testdb", &req).await.unwrap();

    // List roles
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "list": {"list": "roles"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();

    let roles = resp.results["list"].records[0]["roles"].as_array().unwrap();
    assert_eq!(roles.len(), 1);
    assert_eq!(roles[0]["name"], "analyst");
}

#[tokio::test]
async fn test_drop_role() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "cr": {
                "create_role": "temp_role",
                "permissions": []
            }
        }
    })).unwrap();
    shamir.execute("testdb", &req).await.unwrap();

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "dr": {"drop_role": "temp_role"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["dr"].records[0]["existed"], true);
}

// ============================================================================
// Grant/Revoke roles
// ============================================================================

#[tokio::test]
async fn test_grant_and_revoke_role() {
    let shamir = setup_shamir().await;

    // Create user and role
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "user": {"create_user": "alice", "password": "pass", "roles": ["readonly"]},
            "role": {
                "create_role": "analyst",
                "permissions": [
                    {"effect": "allow", "actions": ["read"], "resource": {"scope": "global"}}
                ]
            }
        }
    })).unwrap();
    shamir.execute("testdb", &req).await.unwrap();

    // Grant role
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "grant": {"grant_role": "analyst", "user": "alice"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["grant"].records[0]["granted_role"], "analyst");

    // Verify user has both roles
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 3,
        "queries": {
            "list": {"list": "users"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let users = resp.results["list"].records[0]["users"].as_array().unwrap();
    let alice = &users[0];
    let roles = alice["roles"].as_array().unwrap();
    assert!(roles.contains(&json!("readonly")));
    assert!(roles.contains(&json!("analyst")));

    // Revoke role
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 4,
        "queries": {
            "revoke": {"revoke_role": "analyst", "user": "alice"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["revoke"].records[0]["revoked_role"], "analyst");

    // Verify role removed
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 5,
        "queries": {
            "list": {"list": "users"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let users = resp.results["list"].records[0]["users"].as_array().unwrap();
    let alice = &users[0];
    let roles = alice["roles"].as_array().unwrap();
    assert!(roles.contains(&json!("readonly")));
    assert!(!roles.contains(&json!("analyst")));
}

// ============================================================================
// Role with row-level security (where filter)
// ============================================================================

#[tokio::test]
async fn test_create_role_with_row_filter() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "cr": {
                "create_role": "eu_manager",
                "permissions": [
                    {
                        "effect": "allow",
                        "actions": ["read", "update"],
                        "resource": {
                            "scope": "table",
                            "database": "testdb",
                            "repo": "main",
                            "table": "users"
                        },
                        "where": {"op": "eq", "field": ["region"], "value": "europe"}
                    }
                ]
            }
        }
    })).unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["cr"].records[0]["created_role"], "eu_manager");

    // Verify role stored with where filter
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "list": {"list": "roles"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let roles = resp.results["list"].records[0]["roles"].as_array().unwrap();
    let eu_role = &roles[0];
    assert_eq!(eu_role["name"], "eu_manager");
    // Check that the where filter is present in the permission
    let perms = eu_role["permissions"].as_array().unwrap();
    assert!(perms[0].get("where").is_some());
}

// ============================================================================
// Error cases
// ============================================================================

#[tokio::test]
async fn test_grant_role_to_nonexistent_user() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "grant": {"grant_role": "analyst", "user": "nonexistent"}
        }
    })).unwrap();

    let err = shamir.execute("testdb", &req).await.unwrap_err();
    assert!(matches!(err, crate::db::query::batch::BatchError::QueryError { .. }));
}
