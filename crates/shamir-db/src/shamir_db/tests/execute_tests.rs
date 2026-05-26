//! End-to-end tests for ShamirDb::execute.

use serde_json::json;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::query::batch::BatchRequest;
use crate::ShamirDb;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
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
        "id": 1,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "age": 30},
                    {"name": "Bob", "age": 25}
                ]
            }
        }
    }))
    .unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["ins"].records.len(), 2);
}

#[tokio::test]
async fn test_execute_single_read() {
    let shamir = setup_shamir().await;

    // Seed
    let seed: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "s": {
                "insert_into": "users",
                "values": [{"name": "Alice"}, {"name": "Bob"}],
                "return_result": false
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    // Read
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "users": {"from": "users"}
        }
    }))
    .unwrap();
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
        "id": 1,
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
    }))
    .unwrap();
    shamir.execute("testdb", &q1).await.unwrap();

    // 2. Update: activate Bob
    let q2: BatchRequest = serde_json::from_value(json!({
        "id": 1,
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
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &q2).await.unwrap();
    assert_eq!(resp.results["upd"].records.len(), 1);
    assert_eq!(resp.results["upd"].records[0]["status"], "active");

    // 3. Delete Carol + read remaining
    let q3: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "del": {
                "delete_from": "users",
                "where": {"op": "eq", "field": ["name"], "value": "Carol"}
            },
            "remaining": {"from": "users"}
        }
    }))
    .unwrap();
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
        "id": 1,
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
    }))
    .unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    // Query: find VIP users, then find their orders
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
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
    }))
    .unwrap();

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
        "id": 1,
        "queries": {
            "r": {"from": "users"}
        }
    }))
    .unwrap();

    let err = shamir.execute("nonexistent", &req).await.unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

// ============================================================================
// Migration ops — Phase A stubs return "not yet implemented"
// ============================================================================

#[tokio::test]
async fn test_migration_lifecycle_in_memory() {
    let shamir = setup_shamir().await;

    // Seed some data
    let seed: BatchRequest = serde_json::from_value(json!({
        "id": 0,
        "queries": {
            "s": {
                "insert_into": "users",
                "values": [{"name": "Alice"}, {"name": "Bob"}, {"name": "Carol"}],
                "return_result": false
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    // Start migration: users from main → cold (in_memory)
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "mig": {
                "start_migration": "users",
                "repo": "main",
                "dst_repo": "cold",
                "dst_engine": "in_memory"
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let mig_result = &resp.results["mig"].records[0];
    assert_eq!(mig_result["phase"], "cutover_ready");
    let migration_id = mig_result["migration_id"].as_str().unwrap().to_string();

    // Query status
    let status_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "s": {"migration_status": migration_id}
        }
    }))
    .unwrap();
    let status_resp = shamir.execute("testdb", &status_req).await.unwrap();
    let status = &status_resp.results["s"].records[0];
    assert_eq!(status["phase"], "cutover_ready");
    assert_eq!(status["records_copied"], 3);

    // Commit
    let commit_req: BatchRequest = serde_json::from_value(json!({
        "id": 3,
        "queries": {
            "c": {"commit_migration": migration_id}
        }
    }))
    .unwrap();
    let commit_resp = shamir.execute("testdb", &commit_req).await.unwrap();
    let commit = &commit_resp.results["c"].records[0];
    assert_eq!(commit["phase"], "committed");
    assert_eq!(commit["src_records"], 3);
    assert_eq!(commit["dst_records"], 3);

    // Read from the destination table
    let read_req: BatchRequest = serde_json::from_value(json!({
        "id": 4,
        "queries": {
            "r": {"from": ["cold", "users"]}
        }
    }))
    .unwrap();
    let read_resp = shamir.execute("testdb", &read_req).await.unwrap();
    assert_eq!(read_resp.results["r"].records.len(), 3);
}

#[tokio::test]
async fn test_migration_rollback() {
    let shamir = setup_shamir().await;

    // Seed
    let seed: BatchRequest = serde_json::from_value(json!({
        "id": 0,
        "queries": {
            "s": {
                "insert_into": "users",
                "values": [{"name": "Alice"}],
                "return_result": false
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    // Start migration
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "mig": {
                "start_migration": "users",
                "repo": "main",
                "dst_repo": "rollback_dst",
                "dst_engine": "in_memory"
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let migration_id = resp.results["mig"].records[0]["migration_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Rollback
    let rb_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "r": {"rollback_migration": migration_id}
        }
    }))
    .unwrap();
    let rb_resp = shamir.execute("testdb", &rb_req).await.unwrap();
    assert_eq!(rb_resp.results["r"].records[0]["phase"], "rolled_back");

    // Status should fail (migration removed)
    let status_req: BatchRequest = serde_json::from_value(json!({
        "id": 3,
        "queries": {
            "s": {"migration_status": migration_id}
        }
    }))
    .unwrap();
    let status_err = shamir.execute("testdb", &status_req).await.unwrap_err();
    assert!(matches!(
        status_err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

#[tokio::test]
async fn test_migration_unknown_id() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "c": {"commit_migration": "nonexistent"}
        }
    }))
    .unwrap();
    let err = shamir.execute("testdb", &req).await.unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

// ============================================================================
// Error: unknown repo
// ============================================================================

#[tokio::test]
async fn test_execute_unknown_repo() {
    let shamir = setup_shamir().await;

    // Use a TableRef with a nonexistent repo (array format: ["repo", "table"])
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "r": {
                "from": ["nonexistent", "users"]
            }
        }
    }))
    .unwrap();

    let err = shamir.execute("testdb", &req).await.unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}
