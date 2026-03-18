//! Integration tests for batch executor.

use serde_json::json;

use crate::db::engine::db_instance::db_instance::DbInstance;
use crate::db::engine::repo::repo_types::BoxRepoFactory;
use crate::db::engine::repo::RepoConfig;
use crate::db::engine::table::{TableConfig, TableManager};
use crate::db::query::batch::{
    execute_batch, BatchLimits, BatchOp, BatchRequest, QueryEntry, TableResolver,
};
use crate::db::query::filter::{Filter, FilterValue};
use crate::db::query::read::ReadQuery;
use crate::db::query::write::{DeleteOp, InsertOp, UpdateOp};
use crate::db::DbResult;
use crate::types::common::new_map;

/// Simple resolver that wraps a DbInstance + repo name.
struct TestResolver {
    db: DbInstance,
    repo: String,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(&self, table_name: &str) -> DbResult<TableManager> {
        self.db.get_table(&self.repo, table_name).await
    }
}

async fn setup_resolver() -> TestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![
            TableConfig::new("users"),
            TableConfig::new("orders"),
        ],
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
    let mut queries = new_map();
    queries.insert(
        "insert".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: "users".to_string(),
                values: vec![
                    json!({"name": "Alice", "age": 30}),
                    json!({"name": "Bob", "age": 25}),
                ],
            }),
            return_result: true,
        },
    );
    let insert_req = BatchRequest {
        name: None,
        transactional: false,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };
    let resp = execute_batch(&insert_req, &resolver).await.unwrap();
    assert_eq!(resp.results["insert"].records.len(), 2);

    // Now read
    let mut queries = new_map();
    queries.insert("users".to_string(), ReadQuery::new("users").into());
    let req = BatchRequest {
        name: None,
        transactional: false,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };

    let resp = execute_batch(&req, &resolver).await.unwrap();

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
    let mut seed = new_map();
    seed.insert(
        "s1".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: "users".to_string(),
                values: vec![json!({"name": "Alice"})],
            }),
            return_result: false,
        },
    );
    seed.insert(
        "s2".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: "orders".to_string(),
                values: vec![json!({"item": "Book"})],
            }),
            return_result: false,
        },
    );
    let seed_req = BatchRequest {
        name: None,
        transactional: false,
        queries: seed,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };
    execute_batch(&seed_req, &resolver).await.unwrap();

    // Two independent reads
    let mut queries = new_map();
    queries.insert("users".to_string(), ReadQuery::new("users").into());
    queries.insert("orders".to_string(), ReadQuery::new("orders").into());
    let req = BatchRequest {
        name: None,
        transactional: false,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };

    let resp = execute_batch(&req, &resolver).await.unwrap();

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
    let mut seed = new_map();
    seed.insert(
        "seed".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: "users".to_string(),
                values: vec![
                    json!({"name": "Alice", "status": "active"}),
                    json!({"name": "Bob", "status": "inactive"}),
                    json!({"name": "Carol", "status": "active"}),
                ],
            }),
            return_result: false,
        },
    );
    let seed_req = BatchRequest {
        name: None,
        transactional: false,
        queries: seed,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };
    execute_batch(&seed_req, &resolver).await.unwrap();

    // Query 1: get active users
    // Query 2: get users where name == first active user's name (via $query ref)
    let mut queries = new_map();
    queries.insert(
        "active".to_string(),
        ReadQuery::new("users")
            .filter(Filter::Eq {
                field: vec!["status".into()],
                value: FilterValue::String("active".into()),
            })
            .into(),
    );
    queries.insert(
        "first_active".to_string(),
        QueryEntry {
            op: BatchOp::Read(ReadQuery::new("users").filter(Filter::Eq {
                field: vec!["name".into()],
                value: FilterValue::QueryRef {
                    alias: "active".into(),
                    path: Some("[0].name".into()),
                },
            })),
            return_result: true,
        },
    );
    let req = BatchRequest {
        name: None,
        transactional: false,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };

    let resp = execute_batch(&req, &resolver).await.unwrap();

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

    let mut queries = new_map();
    queries.insert(
        "insert".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: "users".to_string(),
                values: vec![
                    json!({"name": "Alice", "score": 100}),
                    json!({"name": "Bob", "score": 50}),
                ],
            }),
            return_result: true,
        },
    );
    // Read depends on insert implicitly (no $query ref, but same table)
    // Actually this is independent — reads all users after insert completes
    // But since no $query ref, they'll be in the same stage
    // The insert runs first because of map ordering
    queries.insert("read".to_string(), ReadQuery::new("users").into());

    let req = BatchRequest {
        name: None,
        transactional: false,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };

    let resp = execute_batch(&req, &resolver).await.unwrap();

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

    let mut queries = new_map();
    queries.insert(
        "insert".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: "users".to_string(),
                values: vec![json!({"name": "Alice"})],
            }),
            return_result: true,
        },
    );
    queries.insert("read".to_string(), ReadQuery::new("users").into());

    let req = BatchRequest {
        name: None,
        transactional: false,
        queries,
        return_all: true,
        return_only: Some(vec!["read".to_string()]),
        limits: BatchLimits::default(),
    };

    let resp = execute_batch(&req, &resolver).await.unwrap();

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

    let mut queries = new_map();
    queries.insert(
        "setup".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: "users".to_string(),
                values: vec![json!({"name": "Alice"})],
            }),
            return_result: false,
        },
    );
    queries.insert("read".to_string(), ReadQuery::new("users").into());

    let req = BatchRequest {
        name: None,
        transactional: false,
        queries,
        return_all: false, // respect per-entry return_result
        return_only: None,
        limits: BatchLimits::default(),
    };

    let resp = execute_batch(&req, &resolver).await.unwrap();

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
    let mut seed = new_map();
    seed.insert(
        "seed".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: "users".to_string(),
                values: vec![
                    json!({"name": "Alice", "status": "active"}),
                    json!({"name": "Bob", "status": "inactive"}),
                ],
            }),
            return_result: false,
        },
    );
    execute_batch(
        &BatchRequest {
            name: None,
            transactional: false,
            queries: seed,
            return_all: true,
            return_only: None,
            limits: BatchLimits::default(),
        },
        &resolver,
    )
    .await
    .unwrap();

    // Delete inactive, then read
    let mut queries = new_map();
    queries.insert(
        "cleanup".to_string(),
        QueryEntry {
            op: BatchOp::Delete(DeleteOp {
                delete_from: "users".to_string(),
                where_clause: Filter::Eq {
                    field: vec!["status".into()],
                    value: FilterValue::String("inactive".into()),
                },
            }),
            return_result: true,
        },
    );
    let req = BatchRequest {
        name: None,
        transactional: false,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };

    let resp = execute_batch(&req, &resolver).await.unwrap();
    // 1 record deleted (Bob)
    assert_eq!(resp.results["cleanup"].stats.as_ref().unwrap().records_scanned, 1);
}

// ============================================================================
// Circular dependency error
// ============================================================================

#[tokio::test]
async fn test_circular_dependency_error() {
    let resolver = setup_resolver().await;

    let mut queries = new_map();
    // a depends on b, b depends on a
    queries.insert(
        "a".to_string(),
        QueryEntry {
            op: BatchOp::Read(ReadQuery::new("users").filter(Filter::Eq {
                field: vec!["id".into()],
                value: FilterValue::QueryRef {
                    alias: "b".into(),
                    path: Some("[0].id".into()),
                },
            })),
            return_result: true,
        },
    );
    queries.insert(
        "b".to_string(),
        QueryEntry {
            op: BatchOp::Read(ReadQuery::new("users").filter(Filter::Eq {
                field: vec!["id".into()],
                value: FilterValue::QueryRef {
                    alias: "a".into(),
                    path: Some("[0].id".into()),
                },
            })),
            return_result: true,
        },
    );
    let req = BatchRequest {
        name: None,
        transactional: false,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };

    let err = execute_batch(&req, &resolver).await.unwrap_err();
    assert!(matches!(err, crate::db::query::batch::BatchError::CircularDependency { .. }));
}
