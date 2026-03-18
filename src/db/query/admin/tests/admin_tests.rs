//! End-to-end tests for admin (DDL) operations via ShamirDb::execute.

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
        .add_table(TableConfig::new("users"));

    db.add_repo(repo_config).await.unwrap();
    shamir
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
    })).unwrap();

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
    })).unwrap();

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
    })).unwrap();

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
    })).unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["create"].records[0]["created_repo"], "hot_cache");

    // Verify it exists
    let list_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "repos": {"list": "repos"}
        }
    })).unwrap();
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
    })).unwrap();

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
    let seed: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "seed": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "email": "alice@test.com"},
                    {"name": "Bob", "email": "bob@test.com"}
                ]
            }
        }
    })).unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

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
    })).unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["idx"].records[0]["created_index"], "email_idx");
    assert_eq!(resp.results["idx"].records[0]["unique"], true);

    // Now query using the index
    let query: BatchRequest = serde_json::from_value(json!({
        "id": 3,
        "queries": {
            "find": {
                "from": "users",
                "where": {"op": "eq", "field": ["email"], "value": "alice@test.com"}
            }
        }
    })).unwrap();

    let resp = shamir.execute("testdb", &query).await.unwrap();
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
    })).unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["drop"].records[0]["existed"], true);
}

// ============================================================================
// Full DDL + DML pipeline
// ============================================================================

#[tokio::test]
async fn test_ddl_then_dml_pipeline() {
    let shamir = ShamirDb::new().init().await.unwrap();
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
    })).unwrap();
    shamir.execute("app", &setup).await.unwrap();

    // Step 2: Insert data + create index
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
    })).unwrap();
    let resp = shamir.execute("app", &populate).await.unwrap();
    assert_eq!(resp.results["products"].records.len(), 3);

    // Step 3: Query using index
    let query: BatchRequest = serde_json::from_value(json!({
        "id": "query",
        "queries": {
            "cheap": {
                "from": "products",
                "where": {"op": "eq", "field": ["price"], "value": 10}
            }
        }
    })).unwrap();
    let resp = shamir.execute("app", &query).await.unwrap();
    assert_eq!(resp.results["cheap"].records.len(), 1);
    assert_eq!(resp.results["cheap"].records[0]["name"], "Widget");
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
    })).unwrap();

    let err = shamir.execute("testdb", &req).await.unwrap_err();
    assert!(matches!(err, crate::db::query::batch::BatchError::QueryError { .. }));
}
