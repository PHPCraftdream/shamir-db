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

// ============================================================================
// SessionPermissions — unit tests
// ============================================================================

use crate::db::query::auth::{
    Action, Effect, Permission, Resource, Role, SessionPermissions,
};
use crate::db::query::filter::Filter;

#[test]
fn test_superadmin_allows_everything() {
    let roles = vec![Role {
        name: "superadmin".into(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::All],
            resource: Resource::Global,
            row_filter: None,
        }],
    }];
    let session = SessionPermissions::build(&roles);
    assert_eq!(session.check(Action::Read, &Resource::Global), Effect::Allow);
    assert_eq!(
        session.check(
            Action::Delete,
            &Resource::Table {
                database: "x".into(),
                repo: "y".into(),
                table: "z".into(),
            }
        ),
        Effect::Allow
    );
    assert_eq!(
        session.check(Action::ManageUsers, &Resource::Global),
        Effect::Allow
    );
    assert_eq!(
        session.check(Action::ManageRoles, &Resource::Global),
        Effect::Allow
    );
}

#[test]
fn test_no_permissions_denies() {
    let session = SessionPermissions::build(&[]);
    assert_eq!(session.check(Action::Read, &Resource::Global), Effect::Deny);
    assert_eq!(
        session.check(
            Action::Insert,
            &Resource::Table {
                database: "db".into(),
                repo: "main".into(),
                table: "t".into(),
            }
        ),
        Effect::Deny
    );
}

#[test]
fn test_specific_deny_overrides_general_allow() {
    let roles = vec![
        Role {
            name: "a".into(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: Resource::Global,
                row_filter: None,
            }],
        },
        Role {
            name: "b".into(),
            permissions: vec![Permission {
                effect: Effect::Deny,
                actions: vec![Action::Read],
                resource: Resource::Table {
                    database: "db".into(),
                    repo: "main".into(),
                    table: "secrets".into(),
                },
                row_filter: None,
            }],
        },
    ];
    let session = SessionPermissions::build(&roles);
    // General read allowed
    assert_eq!(
        session.check(
            Action::Read,
            &Resource::Table {
                database: "db".into(),
                repo: "main".into(),
                table: "users".into(),
            }
        ),
        Effect::Allow
    );
    // Specific table denied
    assert_eq!(
        session.check(
            Action::Read,
            &Resource::Table {
                database: "db".into(),
                repo: "main".into(),
                table: "secrets".into(),
            }
        ),
        Effect::Deny
    );
}

#[test]
fn test_same_specificity_deny_wins() {
    let roles = vec![
        Role {
            name: "a".into(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Delete],
                resource: Resource::Global,
                row_filter: None,
            }],
        },
        Role {
            name: "b".into(),
            permissions: vec![Permission {
                effect: Effect::Deny,
                actions: vec![Action::Delete],
                resource: Resource::Global,
                row_filter: None,
            }],
        },
    ];
    let session = SessionPermissions::build(&roles);
    assert_eq!(
        session.check(Action::Delete, &Resource::Global),
        Effect::Deny
    );
}

#[test]
fn test_database_level_permission() {
    let roles = vec![Role {
        name: "db_reader".into(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::Read],
            resource: Resource::Database {
                database: "mydb".into(),
            },
            row_filter: None,
        }],
    }];
    let session = SessionPermissions::build(&roles);
    // Allowed within database
    assert_eq!(
        session.check(
            Action::Read,
            &Resource::Table {
                database: "mydb".into(),
                repo: "main".into(),
                table: "users".into(),
            }
        ),
        Effect::Allow
    );
    // Denied for other database
    assert_eq!(
        session.check(
            Action::Read,
            &Resource::Table {
                database: "other".into(),
                repo: "main".into(),
                table: "users".into(),
            }
        ),
        Effect::Deny
    );
    // Write not allowed
    assert_eq!(
        session.check(
            Action::Insert,
            &Resource::Table {
                database: "mydb".into(),
                repo: "main".into(),
                table: "users".into(),
            }
        ),
        Effect::Deny
    );
}

#[test]
fn test_action_all_expands() {
    let roles = vec![Role {
        name: "full".into(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::All],
            resource: Resource::Database {
                database: "mydb".into(),
            },
            row_filter: None,
        }],
    }];
    let session = SessionPermissions::build(&roles);
    assert_eq!(
        session.check(
            Action::Read,
            &Resource::Table {
                database: "mydb".into(),
                repo: "main".into(),
                table: "t".into(),
            }
        ),
        Effect::Allow
    );
    assert_eq!(
        session.check(
            Action::Delete,
            &Resource::Table {
                database: "mydb".into(),
                repo: "main".into(),
                table: "t".into(),
            }
        ),
        Effect::Allow
    );
    assert_eq!(
        session.check(
            Action::ManageRoles,
            &Resource::Database {
                database: "mydb".into(),
            }
        ),
        Effect::Allow
    );
}

#[test]
fn test_row_filter_merging_or() {
    // Two permissions with different row filters on same action+resource → OR
    let roles = vec![
        Role {
            name: "eu".into(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: Resource::Table {
                    database: "db".into(),
                    repo: "main".into(),
                    table: "users".into(),
                },
                row_filter: Some(Filter::Eq {
                    field: vec!["region".into()],
                    value: crate::db::query::filter::FilterValue::String("eu".into()),
                }),
            }],
        },
        Role {
            name: "us".into(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: Resource::Table {
                    database: "db".into(),
                    repo: "main".into(),
                    table: "users".into(),
                },
                row_filter: Some(Filter::Eq {
                    field: vec!["region".into()],
                    value: crate::db::query::filter::FilterValue::String("us".into()),
                }),
            }],
        },
    ];
    let session = SessionPermissions::build(&roles);
    let filter = session.row_filter(
        Action::Read,
        &Resource::Table {
            database: "db".into(),
            repo: "main".into(),
            table: "users".into(),
        },
    );
    assert!(filter.is_some());
    match filter.unwrap() {
        Filter::Or { filters } => assert_eq!(filters.len(), 2),
        other => panic!("Expected Filter::Or, got {:?}", other),
    }
}

#[test]
fn test_row_filter_unrestricted_wins() {
    // One permission with filter + one without → unrestricted (None)
    let roles = vec![
        Role {
            name: "restricted".into(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: Resource::Table {
                    database: "db".into(),
                    repo: "main".into(),
                    table: "users".into(),
                },
                row_filter: Some(Filter::Eq {
                    field: vec!["region".into()],
                    value: crate::db::query::filter::FilterValue::String("eu".into()),
                }),
            }],
        },
        Role {
            name: "full_reader".into(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: Resource::Table {
                    database: "db".into(),
                    repo: "main".into(),
                    table: "users".into(),
                },
                row_filter: None,
            }],
        },
    ];
    let session = SessionPermissions::build(&roles);
    let filter = session.row_filter(
        Action::Read,
        &Resource::Table {
            database: "db".into(),
            repo: "main".into(),
            table: "users".into(),
        },
    );
    assert!(filter.is_none(), "Expected None (unrestricted), got {:?}", filter);
}

#[test]
fn test_check_batch_allows() {
    use crate::db::query::batch::{BatchOp, QueryEntry};
    use crate::db::query::read::ReadQuery;
    use crate::types::common::new_map;

    let roles = vec![Role {
        name: "reader".into(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::Read],
            resource: Resource::Global,
            row_filter: None,
        }],
    }];
    let session = SessionPermissions::build(&roles);

    let mut queries = new_map();
    queries.insert(
        "q1".to_string(),
        QueryEntry {
            op: BatchOp::Read(ReadQuery::new("users")),
            return_result: true,
        },
    );

    assert!(session.check_batch(&queries, "testdb").is_ok());
}

#[test]
fn test_check_batch_denies() {
    use crate::db::query::batch::{BatchOp, QueryEntry};
    use crate::db::query::write::InsertOp;
    use crate::db::query::TableRef;
    use crate::types::common::new_map;

    let roles = vec![Role {
        name: "reader".into(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::Read],
            resource: Resource::Global,
            row_filter: None,
        }],
    }];
    let session = SessionPermissions::build(&roles);

    let mut queries = new_map();
    queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![],
            }),
            return_result: true,
        },
    );

    let result = session.check_batch(&queries, "testdb");
    assert!(result.is_err());
    let (alias, action, _resource) = result.unwrap_err();
    assert_eq!(alias, "ins");
    assert_eq!(action, Action::Insert);
}

#[test]
fn test_repo_level_permission() {
    let roles = vec![Role {
        name: "repo_rw".into(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::Read, Action::Insert, Action::Update],
            resource: Resource::Repo {
                database: "db".into(),
                repo: "main".into(),
            },
            row_filter: None,
        }],
    }];
    let session = SessionPermissions::build(&roles);

    // Allowed: read in main repo
    assert_eq!(
        session.check(
            Action::Read,
            &Resource::Table {
                database: "db".into(),
                repo: "main".into(),
                table: "orders".into(),
            }
        ),
        Effect::Allow
    );
    // Denied: read in different repo
    assert_eq!(
        session.check(
            Action::Read,
            &Resource::Table {
                database: "db".into(),
                repo: "archive".into(),
                table: "orders".into(),
            }
        ),
        Effect::Deny
    );
    // Denied: delete (not granted)
    assert_eq!(
        session.check(
            Action::Delete,
            &Resource::Table {
                database: "db".into(),
                repo: "main".into(),
                table: "orders".into(),
            }
        ),
        Effect::Deny
    );
}

#[test]
fn test_multiple_roles_combined() {
    let roles = vec![
        Role {
            name: "reader".into(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: Resource::Global,
                row_filter: None,
            }],
        },
        Role {
            name: "writer".into(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Insert, Action::Update],
                resource: Resource::Database {
                    database: "app".into(),
                },
                row_filter: None,
            }],
        },
    ];
    let session = SessionPermissions::build(&roles);

    // Read allowed globally
    assert_eq!(
        session.check(Action::Read, &Resource::Global),
        Effect::Allow
    );
    // Insert allowed in "app" db
    assert_eq!(
        session.check(
            Action::Insert,
            &Resource::Table {
                database: "app".into(),
                repo: "main".into(),
                table: "t".into(),
            }
        ),
        Effect::Allow
    );
    // Insert denied in other db
    assert_eq!(
        session.check(
            Action::Insert,
            &Resource::Table {
                database: "other".into(),
                repo: "main".into(),
                table: "t".into(),
            }
        ),
        Effect::Deny
    );
}
