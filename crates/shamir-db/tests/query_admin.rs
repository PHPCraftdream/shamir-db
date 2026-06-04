//! End-to-end tests for admin (DDL) operations via ShamirDb::execute.
//!
//! # Migration note
//!
//! Read/write batches are constructed with `shamir_query_builder`. Admin/DDL
//! ops (`create_repo`, `drop_repo`, `create_table`, `drop_table`,
//! `create_index`, `drop_index`, `list`) stay as raw `json!` because the
//! builder has no coverage for them.

use serde_json::json;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::{BatchRequest, BatchResponse};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;

    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));

    db.add_repo(repo_config).await.unwrap();
    shamir
}

async fn exec_built(shamir: &ShamirDb, req: BatchRequest) -> BatchResponse {
    shamir.execute("testdb", &req).await.unwrap()
}

// ============================================================================
// List operations
// ============================================================================

#[tokio::test]
async fn test_list_databases() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "dbs": {"list": "databases"}
        }
    }))
    .unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    let dbs = &resp.results["dbs"].records[0]["databases"];
    assert!(dbs.as_array().unwrap().contains(&json!("testdb")));
}

#[tokio::test]
async fn test_list_repos() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "repos": {"list": "repos"}
        }
    }))
    .unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    let repos = &resp.results["repos"].records[0]["repos"];
    assert!(repos.as_array().unwrap().contains(&json!("main")));
}

#[tokio::test]
async fn test_list_tables() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "tables": {"list": "tables", "repo": "main"}
        }
    }))
    .unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    let tables = &resp.results["tables"].records[0]["tables"];
    assert!(tables.as_array().unwrap().contains(&json!("users")));
}

// ============================================================================
// Create/drop repo
// ============================================================================

#[tokio::test]
async fn test_create_repo() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "create": {
                "create_repo": "hot_cache",
                "engine": "in_memory",
                "tables": ["sessions", "tokens"]
            }
        }
    }))
    .unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["create"].records[0]["created_repo"],
        "hot_cache"
    );

    // Verify it exists
    let list_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "repos": {"list": "repos"}
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &list_req).await.unwrap();
    let repos = &resp.results["repos"].records[0]["repos"];
    assert!(repos.as_array().unwrap().contains(&json!("hot_cache")));
}

#[tokio::test]
async fn test_drop_repo() {
    let shamir = setup_shamir().await;

    // Create then drop
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "create": {"create_repo": "temp", "engine": "in_memory"},
            "drop": {"drop_repo": "temp"}
        }
    }))
    .unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["drop"].records[0]["existed"], true);
}

// ============================================================================
// Create/drop index
// ============================================================================

#[tokio::test]
async fn test_create_index_via_query() {
    let shamir = setup_shamir().await;

    // Insert data first
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "seed",
        insert("users").rows([
            doc! { "name" => "Alice", "email" => "alice@test.com" },
            doc! { "name" => "Bob", "email" => "bob@test.com" },
        ]),
    );
    exec_built(&shamir, b.build()).await;

    // Create index
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "idx": {
                "create_index": "email_idx",
                "table": "users",
                "fields": [["email"]],
                "unique": true
            }
        }
    }))
    .unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["idx"].records[0]["created_index"], "email_idx");
    assert_eq!(resp.results["idx"].records[0]["unique"], true);

    // Now query using the index
    let mut b = Batch::new();
    b.id(3);
    b.query(
        "find",
        Query::from("users").where_eq("email", "alice@test.com"),
    );
    let resp = exec_built(&shamir, b.build()).await;
    assert_eq!(resp.results["find"].records.len(), 1);
    assert_eq!(resp.results["find"].records[0]["name"], "Alice");
}

#[tokio::test]
async fn test_drop_index_via_query() {
    let shamir = setup_shamir().await;

    // Create then drop index
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "create": {
                "create_index": "name_idx",
                "table": "users",
                "fields": [["name"]]
            },
            "drop": {
                "drop_index": "name_idx",
                "table": "users"
            }
        }
    }))
    .unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["drop"].records[0]["existed"], true);
}

// ============================================================================
// Full DDL + DML pipeline
// ============================================================================

#[tokio::test]
async fn test_ddl_then_dml_pipeline() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("app").await;

    // Step 1: Create repo with tables
    let setup: BatchRequest = serde_json::from_value(json!({
        "id": "setup",
        "queries": {
            "repo": {
                "create_repo": "main",
                "engine": "in_memory",
                "tables": ["products", "orders"]
            }
        }
    }))
    .unwrap();
    shamir.execute("app", &setup).await.unwrap();

    // Step 2: Insert data + create index (mixed admin + write — keep json!)
    let populate: BatchRequest = serde_json::from_value(json!({
        "id": "populate",
        "queries": {
            "products": {
                "insert_into": "products",
                "values": [
                    {"name": "Widget", "price": 10},
                    {"name": "Gadget", "price": 25},
                    {"name": "Widget Pro", "price": 50}
                ]
            },
            "idx": {
                "create_index": "price_idx",
                "table": "products",
                "fields": [["price"]]
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("app", &populate).await.unwrap();
    assert_eq!(resp.results["products"].records.len(), 3);

    // Step 3: Query using index
    let mut b = Batch::new();
    b.id("query");
    b.query("cheap", Query::from("products").where_eq("price", 10));
    let resp = shamir.execute("app", &b.build()).await.unwrap();
    assert_eq!(resp.results["cheap"].records.len(), 1);
    assert_eq!(resp.results["cheap"].records[0]["name"], "Widget");
}

// ============================================================================
// List indexes
// ============================================================================

#[tokio::test]
async fn test_list_indexes() {
    let shamir = setup_shamir().await;

    // Seed + create indexes (mixed admin + write — keep json!)
    let setup: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "seed": {
                "insert_into": "users",
                "values": [{"name": "Alice", "email": "a@test.com"}]
            },
            "idx1": {
                "create_index": "name_idx",
                "table": "users",
                "fields": [["name"]]
            },
            "idx2": {
                "create_index": "email_idx",
                "table": "users",
                "fields": [["email"]],
                "unique": true
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &setup).await.unwrap();

    // List indexes
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "idxs": {"list": "indexes", "table": "users"}
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();

    let indexes = resp.results["idxs"].records[0]["indexes"]
        .as_array()
        .unwrap();
    assert_eq!(indexes.len(), 2);

    // Check we have both regular and unique
    let names: Vec<&str> = indexes
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"name_idx"));
    assert!(names.contains(&"email_idx"));

    // Check unique flags
    let email_idx = indexes.iter().find(|i| i["name"] == "email_idx").unwrap();
    assert_eq!(email_idx["unique"], true);
    let name_idx = indexes.iter().find(|i| i["name"] == "name_idx").unwrap();
    assert_eq!(name_idx["unique"], false);
}

// ============================================================================
// Create/drop table — actually works
// ============================================================================

#[tokio::test]
async fn test_create_table_then_use_it() {
    let shamir = setup_shamir().await;

    // Create a new table via DDL
    let create: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "ct": {"create_table": "products", "repo": "main"}
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &create).await.unwrap();
    assert_eq!(resp.results["ct"].records[0]["created_table"], "products");

    // Verify it appears in list
    let list: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "tables": {"list": "tables", "repo": "main"}
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &list).await.unwrap();
    let tables = resp.results["tables"].records[0]["tables"]
        .as_array()
        .unwrap();
    assert!(tables.contains(&json!("products")));

    // Actually insert data into the new table
    let mut b = Batch::new();
    b.id(3);
    b.insert(
        "ins",
        insert("products").rows([
            doc! { "name" => "Widget", "price" => 10 },
            doc! { "name" => "Gadget", "price" => 25 },
        ]),
    );
    let resp = exec_built(&shamir, b.build()).await;
    assert_eq!(resp.results["ins"].records.len(), 2);

    // Read back
    let mut b = Batch::new();
    b.id(4);
    b.query("all", Query::from("products"));
    let resp = exec_built(&shamir, b.build()).await;
    assert_eq!(resp.results["all"].records.len(), 2);
}

#[tokio::test]
async fn test_drop_table() {
    let shamir = setup_shamir().await;

    // Drop existing table
    let drop: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "dt": {"drop_table": "users", "repo": "main"}
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &drop).await.unwrap();
    assert_eq!(resp.results["dt"].records[0]["existed"], true);

    // Verify it's gone — insert should fail with table not found
    let mut b = Batch::new();
    b.id(2);
    b.insert("ins", insert("users").row(doc! { "name" => "Alice" }));
    let err = shamir.execute("testdb", &b.build()).await.unwrap_err();
    assert!(matches!(
        err,
        shamir_db::query::batch::BatchError::QueryError { .. }
    ));
}

#[tokio::test]
async fn test_drop_nonexistent_table() {
    let shamir = setup_shamir().await;

    let drop: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "dt": {"drop_table": "nonexistent", "repo": "main"}
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &drop).await.unwrap();
    assert_eq!(resp.results["dt"].records[0]["existed"], false);
}

// ============================================================================
// Error cases
// ============================================================================

#[tokio::test]
async fn test_admin_unknown_repo_error() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "tables": {"list": "tables", "repo": "nonexistent"}
        }
    }))
    .unwrap();

    let err = shamir.execute("testdb", &req).await.unwrap_err();
    assert!(matches!(
        err,
        shamir_db::query::batch::BatchError::QueryError { .. }
    ));
}
