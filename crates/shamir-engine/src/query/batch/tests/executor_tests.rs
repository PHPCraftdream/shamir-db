//! Integration tests for batch executor.

use serde_json::json;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch, BatchRequest, QueryRunner, TableResolver};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbResult;

/// Simple resolver that wraps a DbInstance + repo name.
struct TestResolver {
    db: DbInstance,
    repo: String,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table(&self.repo, &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<crate::repo::RepoInstance> {
        Err(shamir_storage::error::DbError::NotFound(
            "TestResolver does not back transactional repo lookups".into(),
        ))
    }
}

async fn setup_resolver() -> TestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolver {
        db,
        repo: "default".to_string(),
    }
}

// ============================================================================
// Single read query
// ============================================================================

#[tokio::test]
async fn test_single_read_query() {
    let resolver = setup_resolver().await;

    // Insert some data first
    let insert_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "insert": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "age": 30},
                    {"name": "Bob", "age": 25}
                ]
            }
        }
    }))
    .unwrap();
    let resp = execute_batch(&insert_req, &resolver, None).await.unwrap();
    assert_eq!(resp.results["insert"].records.len(), 2);

    // Now read
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "users": {"from": "users"}
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results["users"].records.len(), 2);
    assert!(!resp.execution_plan.is_empty());
}

// ============================================================================
// Independent queries run in same stage
// ============================================================================

#[tokio::test]
async fn test_independent_queries_same_stage() {
    let resolver = setup_resolver().await;

    // Seed data
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "s1": {
                "insert_into": "users",
                "values": [{"name": "Alice"}],
                "return_result": false
            },
            "s2": {
                "insert_into": "orders",
                "values": [{"item": "Book"}],
                "return_result": false
            }
        }
    }))
    .unwrap();
    execute_batch(&seed_req, &resolver, None).await.unwrap();

    // Two independent reads
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "users": {"from": "users"},
            "orders": {"from": "orders"}
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

    // Both in same stage (no dependencies)
    assert_eq!(resp.execution_plan.len(), 1);
    assert_eq!(resp.execution_plan[0].len(), 2);
    assert_eq!(resp.results.len(), 2);
}

// ============================================================================
// Dependent queries: $query ref
// ============================================================================

#[tokio::test]
async fn test_dependent_query_ref() {
    let resolver = setup_resolver().await;

    // Seed users
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "seed": {
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
    execute_batch(&seed_req, &resolver, None).await.unwrap();

    // Query 1: get active users
    // Query 2: get users where name == first active user's name (via $query ref)
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "active": {
                "from": "users",
                "where": {"op": "eq", "field": ["status"], "value": "active"}
            },
            "first_active": {
                "from": "users",
                "where": {
                    "op": "eq",
                    "field": ["name"],
                    "value": {"$query": "active", "path": "[0].name"}
                }
            }
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

    // Two stages: [active], [first_active]
    assert_eq!(resp.execution_plan.len(), 2);
    assert_eq!(resp.results["active"].records.len(), 2); // Alice + Carol
    assert_eq!(resp.results["first_active"].records.len(), 1); // Alice
}

// ============================================================================
// Insert + read pipeline
// ============================================================================

#[tokio::test]
async fn test_insert_then_read() {
    let resolver = setup_resolver().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "insert": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "score": 100},
                    {"name": "Bob", "score": 50}
                ]
            },
            "read": {"from": "users"}
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

    // Both in same stage (no explicit dependency)
    assert_eq!(resp.results["insert"].records.len(), 2);
    // Read may or may not see the inserted records depending on execution order
    // within the stage (sequential currently, so insert runs first)
    assert_eq!(resp.results["read"].records.len(), 2);
}

// ============================================================================
// return_only filtering
// ============================================================================

#[tokio::test]
async fn test_return_only() {
    let resolver = setup_resolver().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "insert": {
                "insert_into": "users",
                "values": [{"name": "Alice"}]
            },
            "read": {"from": "users"}
        },
        "return_only": ["read"]
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

    // Only "read" returned
    assert_eq!(resp.results.len(), 1);
    assert!(resp.results.contains_key("read"));
}

// ============================================================================
// return_result = false
// ============================================================================

#[tokio::test]
async fn test_return_result_false() {
    let resolver = setup_resolver().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "setup": {
                "insert_into": "users",
                "values": [{"name": "Alice"}],
                "return_result": false
            },
            "read": {"from": "users"}
        },
        "return_all": false
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

    // "setup" has return_result=false, "read" has return_result=true (default)
    assert_eq!(resp.results.len(), 1);
    assert!(resp.results.contains_key("read"));
}

// ============================================================================
// Delete in batch
// ============================================================================

#[tokio::test]
async fn test_batch_with_delete() {
    let resolver = setup_resolver().await;

    // Seed
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "seed": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "status": "active"},
                    {"name": "Bob", "status": "inactive"}
                ],
                "return_result": false
            }
        }
    }))
    .unwrap();
    execute_batch(&seed_req, &resolver, None).await.unwrap();

    // Delete inactive, then read
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "cleanup": {
                "delete_from": "users",
                "where": {"op": "eq", "field": ["status"], "value": "inactive"}
            }
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None).await.unwrap();
    // 1 record deleted (Bob)
    assert_eq!(
        resp.results["cleanup"]
            .stats
            .as_ref()
            .unwrap()
            .records_scanned,
        1
    );
}

// ============================================================================
// Circular dependency error
// ============================================================================

#[tokio::test]
async fn test_circular_dependency_error() {
    let resolver = setup_resolver().await;

    // a depends on b, b depends on a
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "a": {
                "from": "users",
                "where": {
                    "op": "eq",
                    "field": ["id"],
                    "value": {"$query": "b", "path": "[0].id"}
                }
            },
            "b": {
                "from": "users",
                "where": {
                    "op": "eq",
                    "field": ["id"],
                    "value": {"$query": "a", "path": "[0].id"}
                }
            }
        }
    }))
    .unwrap();

    let err = execute_batch(&req, &resolver, None).await.unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::CircularDependency { .. }
    ));
}

// ============================================================================
// Pre-validation: unknown table fails before execution
// ============================================================================

#[tokio::test]
async fn test_unknown_table_fails_early() {
    let resolver = setup_resolver().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "good": {
                "insert_into": "users",
                "values": [{"name": "Alice"}]
            },
            "bad": {"from": "nonexistent_table"}
        }
    }))
    .unwrap();

    let err = execute_batch(&req, &resolver, None).await.unwrap_err();
    // Should fail with table not found error BEFORE any execution
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

// ============================================================================
// Request ID echoed in response
// ============================================================================

#[tokio::test]
async fn test_request_id_echoed() {
    let resolver = setup_resolver().await;

    // String ID
    let req: BatchRequest = serde_json::from_value(json!({
        "id": "req-42",
        "queries": {
            "q": {"from": "users"}
        }
    }))
    .unwrap();
    let resp = execute_batch(&req, &resolver, None).await.unwrap();
    assert_eq!(resp.id, json!("req-42"));

    // Numeric ID
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 123,
        "queries": {
            "q": {"from": "users"}
        }
    }))
    .unwrap();
    let resp = execute_batch(&req, &resolver, None).await.unwrap();
    assert_eq!(resp.id, json!(123));
}

// ============================================================================
// QueryRunner struct — tx: None path
// ============================================================================

#[tokio::test]
async fn test_query_runner_none_tx_insert_and_read() {
    let resolver = setup_resolver().await;

    // Insert via QueryRunner with tx: None
    let insert_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [{"name": "Eve", "age": 28}]
            }
        }
    }))
    .unwrap();
    let insert_entry = insert_req.queries.get("ins").unwrap().clone();
    let mut runner = QueryRunner {
        resolver: &resolver,
        admin: None,
        tx: None,
    };
    let result = runner
        .run(
            "ins",
            &insert_entry,
            &shamir_types::types::common::new_map(),
        )
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);

    // Read via QueryRunner with tx: None
    let read_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "q": {"from": "users"}
        }
    }))
    .unwrap();
    let read_entry = read_req.queries.get("q").unwrap().clone();
    let mut runner = QueryRunner {
        resolver: &resolver,
        admin: None,
        tx: None,
    };
    let result = runner
        .run("q", &read_entry, &shamir_types::types::common::new_map())
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
}

// ============================================================================
// execute_batch — transactional SI happy path
// ============================================================================

struct TxTestResolver {
    repo: crate::repo::RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for TxTestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<crate::repo::RepoInstance> {
        Ok(self.repo.clone())
    }
}

#[tokio::test]
async fn execute_batch_transactional_si_happy_path() {
    use futures::StreamExt;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();

    let resolver = TxTestResolver { repo: repo.clone() };

    let request: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "transactional": true,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "alice"},
                    {"name": "bob"}
                ]
            }
        },
        "return_all": true
    }))
    .unwrap();

    let response = execute_batch(&request, &resolver, None).await.unwrap();

    let info = response.transaction.expect("transaction info present");
    assert_eq!(info.status, "committed");
    assert!(info.tx_id > 0);
    assert!(info.commit_version.unwrap_or(0) > 0);

    // Outside the tx, observer reads see the committed records.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(count, 2, "outside observer must see 2 committed records");
}
