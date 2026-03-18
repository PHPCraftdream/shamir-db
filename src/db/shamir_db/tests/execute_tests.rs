//! End-to-end tests for ShamirDb::execute.

use serde_json::json;

use crate::db::engine::repo::repo_types::BoxRepoFactory;
use crate::db::engine::repo::RepoConfig;
use crate::db::engine::table::TableConfig;
use crate::db::query::batch::BatchRequest;
use crate::db::ShamirDb;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::new().init().await.unwrap();
    let db = shamir.create_db("testdb").await;

    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"))
        .add_table(TableConfig::new("orders"));

    db.add_repo(repo_config).await.unwrap();
    shamir
}

// ============================================================================
// Basic single operations
// ============================================================================

#[tokio::test]
async fn test_execute_single_insert() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "age": 30},
                    {"name": "Bob", "age": 25}
                ]
            }
        }
    })).unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["ins"].records.len(), 2);
}

#[tokio::test]
async fn test_execute_single_read() {
    let shamir = setup_shamir().await;

    // Seed
    let seed: BatchRequest = serde_json::from_value(json!({
        "queries": {
            "s": {
                "insert_into": "users",
                "values": [{"name": "Alice"}, {"name": "Bob"}],
                "return_result": false
            }
        }
    })).unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    // Read
    let req: BatchRequest = serde_json::from_value(json!({
        "queries": {
            "users": {"from": "users"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();

    assert_eq!(resp.results["users"].records.len(), 2);
}

// ============================================================================
// Full CRUD pipeline in one batch
// ============================================================================

#[tokio::test]
async fn test_execute_crud_pipeline() {
    let shamir = setup_shamir().await;

    // 1. Insert users
    let q1: BatchRequest = serde_json::from_value(json!({
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "status": "active"},
                    {"name": "Bob", "status": "inactive"},
                    {"name": "Carol", "status": "active"}
                ],
                "return_result": false
            }
        }
    })).unwrap();
    shamir.execute("testdb", &q1).await.unwrap();

    // 2. Update: activate Bob
    let q2: BatchRequest = serde_json::from_value(json!({
        "queries": {
            "upd": {
                "update": "users",
                "where": {"op": "eq", "field": ["name"], "value": "Bob"},
                "set": {"status": "active"},
                "select": {
                    "return_mode": "changed"
                }
            }
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &q2).await.unwrap();
    assert_eq!(resp.results["upd"].records.len(), 1);
    assert_eq!(resp.results["upd"].records[0]["status"], "active");

    // 3. Delete Carol + read remaining
    let q3: BatchRequest = serde_json::from_value(json!({
        "queries": {
            "del": {
                "delete_from": "users",
                "where": {"op": "eq", "field": ["name"], "value": "Carol"}
            },
            "remaining": {"from": "users"}
        }
    })).unwrap();
    let resp = shamir.execute("testdb", &q3).await.unwrap();

    assert_eq!(resp.results["remaining"].records.len(), 2);
}

// ============================================================================
// Multi-table batch with $query dependency
// ============================================================================

#[tokio::test]
async fn test_execute_multi_table_with_dependency() {
    let shamir = setup_shamir().await;

    // Seed users and orders
    let seed: BatchRequest = serde_json::from_value(json!({
        "queries": {
            "s1": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "tier": "vip"},
                    {"name": "Bob", "tier": "basic"}
                ],
                "return_result": false
            },
            "s2": {
                "insert_into": "orders",
                "values": [
                    {"user": "Alice", "amount": 100},
                    {"user": "Bob", "amount": 50},
                    {"user": "Alice", "amount": 200}
                ],
                "return_result": false
            }
        }
    })).unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    // Query: find VIP users, then find their orders
    let req: BatchRequest = serde_json::from_value(json!({
        "queries": {
            "vips": {
                "from": "users",
                "where": {"op": "eq", "field": ["tier"], "value": "vip"}
            },
            "vip_orders": {
                "from": "orders",
                "where": {
                    "op": "eq",
                    "field": ["user"],
                    "value": {"$query": "vips", "path": "[0].name"}
                }
            }
        }
    })).unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();

    // Stage 1: vips → Alice
    assert_eq!(resp.results["vips"].records.len(), 1);
    // Stage 2: vip_orders → Alice's orders (2)
    assert_eq!(resp.results["vip_orders"].records.len(), 2);
    assert_eq!(resp.execution_plan.len(), 2);
}

// ============================================================================
// Error: unknown database
// ============================================================================

#[tokio::test]
async fn test_execute_unknown_db() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "queries": {
            "r": {"from": "users"}
        }
    })).unwrap();

    let err = shamir
        .execute("nonexistent", &req)
        .await
        .unwrap_err();
    assert!(matches!(err, crate::db::query::batch::BatchError::QueryError { .. }));
}

// ============================================================================
// Error: unknown repo
// ============================================================================

#[tokio::test]
async fn test_execute_unknown_repo() {
    let shamir = setup_shamir().await;

    // Use a TableRef with a nonexistent repo (array format: ["repo", "table"])
    let req: BatchRequest = serde_json::from_value(json!({
        "queries": {
            "r": {
                "from": ["nonexistent", "users"]
            }
        }
    })).unwrap();

    let err = shamir
        .execute("testdb", &req)
        .await
        .unwrap_err();
    assert!(matches!(err, crate::db::query::batch::BatchError::QueryError { .. }));
}
