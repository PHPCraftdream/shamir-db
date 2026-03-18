//! End-to-end tests for ShamirDb::execute.

use serde_json::json;

use crate::db::engine::repo::repo_types::BoxRepoFactory;
use crate::db::engine::repo::RepoConfig;
use crate::db::engine::table::TableConfig;
use crate::db::query::batch::{BatchLimits, BatchOp, BatchRequest, QueryEntry};
use crate::db::query::filter::{Filter, FilterValue};
use crate::db::query::read::ReadQuery;
use crate::db::query::write::{DeleteOp, InsertOp, UpdateOp, UpdateSelect, UpdateReturnMode};
use crate::db::query::TableRef;
use crate::db::ShamirDb;
use crate::types::common::new_map;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::new().init().await.unwrap();
    let db = shamir.create_db("testdb").await;

    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"))
        .add_table(TableConfig::new("orders"));

    db.add_repo(repo_config).await.unwrap();
    shamir
}

fn batch(queries: crate::types::common::TMap<String, QueryEntry>) -> BatchRequest {
    BatchRequest {
        name: None,
        transactional: false,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    }
}

// ============================================================================
// Basic single operations
// ============================================================================

#[tokio::test]
async fn test_execute_single_insert() {
    let shamir = setup_shamir().await;

    let mut q = new_map();
    q.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![
                    json!({"name": "Alice", "age": 30}),
                    json!({"name": "Bob", "age": 25}),
                ],
            }),
            return_result: true,
        },
    );

    let resp = shamir.execute("testdb", &batch(q)).await.unwrap();
    assert_eq!(resp.results["ins"].records.len(), 2);
}

#[tokio::test]
async fn test_execute_single_read() {
    let shamir = setup_shamir().await;

    // Seed
    let mut seed = new_map();
    seed.insert(
        "s".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![json!({"name": "Alice"}), json!({"name": "Bob"})],
            }),
            return_result: false,
        },
    );
    shamir.execute("testdb", &batch(seed)).await.unwrap();

    // Read
    let mut q = new_map();
    q.insert("users".to_string(), ReadQuery::new("users").into());
    let resp = shamir.execute("testdb", &batch(q)).await.unwrap();

    assert_eq!(resp.results["users"].records.len(), 2);
}

// ============================================================================
// Full CRUD pipeline in one batch
// ============================================================================

#[tokio::test]
async fn test_execute_crud_pipeline() {
    let shamir = setup_shamir().await;

    // 1. Insert users
    let mut q1 = new_map();
    q1.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![
                    json!({"name": "Alice", "status": "active"}),
                    json!({"name": "Bob", "status": "inactive"}),
                    json!({"name": "Carol", "status": "active"}),
                ],
            }),
            return_result: false,
        },
    );
    shamir.execute("testdb", &batch(q1)).await.unwrap();

    // 2. Update: activate Bob
    let mut q2 = new_map();
    q2.insert(
        "upd".to_string(),
        QueryEntry {
            op: BatchOp::Update(UpdateOp {
                update: TableRef::new("users"),
                where_clause: Some(Filter::Eq {
                    field: vec!["name".into()],
                    value: FilterValue::String("Bob".into()),
                }),
                set: json!({"status": "active"}),
                select: Some(UpdateSelect {
                    return_mode: UpdateReturnMode::Changed,
                    fields: None,
                }),
            }),
            return_result: true,
        },
    );
    let resp = shamir.execute("testdb", &batch(q2)).await.unwrap();
    assert_eq!(resp.results["upd"].records.len(), 1);
    assert_eq!(resp.results["upd"].records[0]["status"], "active");

    // 3. Delete Carol + read remaining
    let mut q3 = new_map();
    q3.insert(
        "del".to_string(),
        QueryEntry {
            op: BatchOp::Delete(DeleteOp {
                delete_from: TableRef::new("users"),
                where_clause: Filter::Eq {
                    field: vec!["name".into()],
                    value: FilterValue::String("Carol".into()),
                },
            }),
            return_result: true,
        },
    );
    q3.insert("remaining".to_string(), ReadQuery::new("users").into());
    let resp = shamir.execute("testdb", &batch(q3)).await.unwrap();

    assert_eq!(resp.results["remaining"].records.len(), 2);
}

// ============================================================================
// Multi-table batch with $query dependency
// ============================================================================

#[tokio::test]
async fn test_execute_multi_table_with_dependency() {
    let shamir = setup_shamir().await;

    // Seed users and orders
    let mut seed = new_map();
    seed.insert(
        "s1".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![
                    json!({"name": "Alice", "tier": "vip"}),
                    json!({"name": "Bob", "tier": "basic"}),
                ],
            }),
            return_result: false,
        },
    );
    seed.insert(
        "s2".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![
                    json!({"user": "Alice", "amount": 100}),
                    json!({"user": "Bob", "amount": 50}),
                    json!({"user": "Alice", "amount": 200}),
                ],
            }),
            return_result: false,
        },
    );
    shamir.execute("testdb", &batch(seed)).await.unwrap();

    // Query: find VIP users, then find their orders
    let mut q = new_map();
    q.insert(
        "vips".to_string(),
        ReadQuery::new("users")
            .filter(Filter::Eq {
                field: vec!["tier".into()],
                value: FilterValue::String("vip".into()),
            })
            .into(),
    );
    q.insert(
        "vip_orders".to_string(),
        QueryEntry {
            op: BatchOp::Read(ReadQuery::new("orders").filter(Filter::Eq {
                field: vec!["user".into()],
                value: FilterValue::QueryRef {
                    alias: "vips".into(),
                    path: Some("[0].name".into()),
                },
            })),
            return_result: true,
        },
    );

    let resp = shamir.execute("testdb", &batch(q)).await.unwrap();

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

    let mut q = new_map();
    q.insert("r".to_string(), ReadQuery::new("users").into());

    let err = shamir
        .execute("nonexistent", &batch(q))
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

    // Use a TableRef with a nonexistent repo
    let mut q = new_map();
    let mut read = ReadQuery::new("users");
    read.from = TableRef::with_repo("nonexistent", "users");
    q.insert("r".to_string(), QueryEntry { op: BatchOp::Read(read), return_result: true });

    let err = shamir
        .execute("testdb", &batch(q))
        .await
        .unwrap_err();
    assert!(matches!(err, crate::db::query::batch::BatchError::QueryError { .. }));
}
