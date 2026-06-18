//! End-to-end tests for auth operations via ShamirDb::execute.

use serde_json::json;
use shamir_types::mpack;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::auth::{Action, Effect, Permission, Resource, Role, SessionPermissions};
use shamir_db::query::filter::Filter;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

// ============================================================================
// Users CRUD
// ============================================================================

#[tokio::test]
async fn test_create_user() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_user(
        "cu",
        ddl::create_user("alice", "secret123")
            .roles(["readonly"])
            .profile(mpack!({
                "department": "engineering",
                "level": 3
            })),
    );
    let req = b.to_request_via_msgpack();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["cu"].records[0].get_value_str("created_user"),
        Some("alice")
    );
}

#[tokio::test]
async fn test_list_users() {
    let shamir = setup_shamir().await;

    // Create two users
    let mut b = Batch::new();
    b.id(1);
    b.create_user("u1", ddl::create_user("alice", "pass1").roles(["readonly"]));
    b.create_user("u2", ddl::create_user("bob", "pass2").roles(["readwrite"]));
    let req = b.to_request_via_msgpack();
    shamir.execute("testdb", &req).await.unwrap();

    // List users
    let mut b = Batch::new();
    b.id(2);
    b.list_users("list", ddl::list_users());
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();

    let rec = serde_json::Value::from(resp.results["list"].records[0].as_value().into_owned());
    let users = rec["users"].as_array().unwrap();
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
    let mut b = Batch::new();
    b.id(1);
    b.create_user("cu", ddl::create_user("alice", "pass"));
    let req = b.to_request_via_msgpack();
    shamir.execute("testdb", &req).await.unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.drop_user("du", ddl::drop_user("alice"));
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["du"].records[0].get_value_bool("existed"),
        Some(true)
    );

    // Drop non-existent
    let mut b = Batch::new();
    b.id(3);
    b.drop_user("du", ddl::drop_user("alice"));
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["du"].records[0].get_value_bool("existed"),
        Some(false)
    );
}

// ============================================================================
// Roles CRUD
// ============================================================================

#[tokio::test]
async fn test_create_role() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_role(
        "cr",
        ddl::create_role(
            "analyst",
            vec![
                Permission {
                    effect: Effect::Allow,
                    actions: vec![Action::Read],
                    resource: Resource::Global,
                    row_filter: None,
                },
                Permission {
                    effect: Effect::Allow,
                    actions: vec![Action::Insert, Action::Update],
                    resource: Resource::Table {
                        database: "mydb".into(),
                        repo: "main".into(),
                        table: "reports".into(),
                    },
                    row_filter: None,
                },
            ],
        ),
    );
    let req = b.to_request_via_msgpack();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["cr"].records[0].get_value_str("created_role"),
        Some("analyst")
    );
}

#[tokio::test]
async fn test_list_roles() {
    let shamir = setup_shamir().await;

    // Create role
    let mut b = Batch::new();
    b.id(1);
    b.create_role(
        "cr",
        ddl::create_role(
            "analyst",
            vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: Resource::Global,
                row_filter: None,
            }],
        ),
    );
    let req = b.to_request_via_msgpack();
    shamir.execute("testdb", &req).await.unwrap();

    // List roles
    let mut b = Batch::new();
    b.id(2);
    b.list_roles("list", ddl::list_roles());
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();

    let rec = serde_json::Value::from(resp.results["list"].records[0].as_value().into_owned());
    let roles = rec["roles"].as_array().unwrap();
    assert_eq!(roles.len(), 1);
    assert_eq!(roles[0]["name"], "analyst");
}

#[tokio::test]
async fn test_drop_role() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_role("cr", ddl::create_role("temp_role", vec![]));
    let req = b.to_request_via_msgpack();
    shamir.execute("testdb", &req).await.unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.drop_role("dr", ddl::drop_role("temp_role"));
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["dr"].records[0].get_value_bool("existed"),
        Some(true)
    );
}

// ============================================================================
// Grant/Revoke roles
// ============================================================================

#[tokio::test]
async fn test_grant_and_revoke_role() {
    let shamir = setup_shamir().await;

    // Create user and role
    let mut b = Batch::new();
    b.id(1);
    b.create_user(
        "user",
        ddl::create_user("alice", "pass").roles(["readonly"]),
    );
    b.create_role(
        "role",
        ddl::create_role(
            "analyst",
            vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: Resource::Global,
                row_filter: None,
            }],
        ),
    );
    let req = b.to_request_via_msgpack();
    shamir.execute("testdb", &req).await.unwrap();

    // Grant role
    let mut b = Batch::new();
    b.id(2);
    b.grant_role("grant", ddl::grant_role("analyst", "alice"));
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["grant"].records[0].get_value_str("granted_role"),
        Some("analyst")
    );

    // Verify user has both roles
    let mut b = Batch::new();
    b.id(3);
    b.list_users("list", ddl::list_users());
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let rec = serde_json::Value::from(resp.results["list"].records[0].as_value().into_owned());
    let users = rec["users"].as_array().unwrap();
    let alice = &users[0];
    let roles = alice["roles"].as_array().unwrap();
    assert!(roles.contains(&json!("readonly")));
    assert!(roles.contains(&json!("analyst")));

    // Revoke role
    let mut b = Batch::new();
    b.id(4);
    b.revoke_role("revoke", ddl::revoke_role("analyst", "alice"));
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["revoke"].records[0].get_value_str("revoked_role"),
        Some("analyst")
    );

    // Verify role removed
    let mut b = Batch::new();
    b.id(5);
    b.list_users("list", ddl::list_users());
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let rec = serde_json::Value::from(resp.results["list"].records[0].as_value().into_owned());
    let users = rec["users"].as_array().unwrap();
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

    use shamir_db::query::filter::FilterValue;

    let mut b = Batch::new();
    b.id(1);
    b.create_role(
        "cr",
        ddl::create_role(
            "eu_manager",
            vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read, Action::Update],
                resource: Resource::Table {
                    database: "testdb".into(),
                    repo: "main".into(),
                    table: "users".into(),
                },
                row_filter: Some(Filter::Eq {
                    field: vec!["region".into()],
                    value: FilterValue::String("europe".into()),
                }),
            }],
        ),
    );
    let req = b.to_request_via_msgpack();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["cr"].records[0].get_value_str("created_role"),
        Some("eu_manager")
    );

    // Verify role stored with where filter
    let mut b = Batch::new();
    b.id(2);
    b.list_roles("list", ddl::list_roles());
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let rec = serde_json::Value::from(resp.results["list"].records[0].as_value().into_owned());
    let roles = rec["roles"].as_array().unwrap();
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

    let mut b = Batch::new();
    b.id(1);
    b.grant_role("grant", ddl::grant_role("analyst", "nonexistent"));
    let req = b.to_request_via_msgpack();

    let err = shamir.execute("testdb", &req).await.unwrap_err();
    assert!(matches!(
        err,
        shamir_db::query::batch::BatchError::QueryError { .. }
    ));
}

// ============================================================================
// SessionPermissions — unit tests
// ============================================================================

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
    assert_eq!(
        session.check(Action::Read, &Resource::Global),
        Effect::Allow
    );
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
                    value: shamir_db::query::filter::FilterValue::String("eu".into()),
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
                    value: shamir_db::query::filter::FilterValue::String("us".into()),
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
                    value: shamir_db::query::filter::FilterValue::String("eu".into()),
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
    assert!(
        filter.is_none(),
        "Expected None (unrestricted), got {:?}",
        filter
    );
}

#[test]
fn test_check_batch_allows() {
    use shamir_db::query::batch::{BatchOp, QueryEntry};
    use shamir_db::query::read::ReadQuery;
    use shamir_types::types::common::new_map;

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
            after: Vec::new(),
        },
    );

    assert!(session.check_batch(&queries, "testdb").is_ok());
}

#[test]
fn test_check_batch_denies() {
    use shamir_db::query::batch::{BatchOp, QueryEntry};
    use shamir_db::query::write::InsertOp;
    use shamir_db::query::TableRef;
    use shamir_types::types::common::new_map;

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
                records_idmsgpack: Vec::new(),
            }),
            return_result: true,
            after: Vec::new(),
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
