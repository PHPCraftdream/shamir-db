//! End-to-end tests for admin (DDL) operations via ShamirDb::execute.
//!
//! All batch requests are built with `shamir_query_builder` and round-tripped
//! through MessagePack to mirror the real wire path.

use serde_json::json;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::{BatchRequest, BatchResponse};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;

fn to_req(b: &Batch) -> BatchRequest {
    let bytes = b.to_msgpack().expect("msgpack encode");
    rmp_serde::from_slice(&bytes).expect("msgpack decode")
}

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

    let mut b = Batch::new();
    b.id(1);
    b.list_databases("dbs", ddl::list_databases());
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();

    let dbs = &resp.results["dbs"].records[0]["databases"];
    assert!(dbs.as_array().unwrap().contains(&json!("testdb")));
}

#[tokio::test]
async fn test_list_repos() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.list_repos("repos", ddl::list_repos());
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();

    let repos = &resp.results["repos"].records[0]["repos"];
    assert!(repos.as_array().unwrap().contains(&json!("main")));
}

#[tokio::test]
async fn test_list_tables() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.list_tables("tables", ddl::list_tables().repo("main"));
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();

    let tables = &resp.results["tables"].records[0]["tables"];
    assert!(tables.as_array().unwrap().contains(&json!("users")));
}

// ============================================================================
// Create/drop repo
// ============================================================================

#[tokio::test]
async fn test_create_repo() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_repo(
        "create",
        ddl::create_repo("hot_cache")
            .engine("in_memory")
            .tables(["sessions", "tokens"]),
    );
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();
    assert_eq!(
        resp.results["create"].records[0]["created_repo"],
        "hot_cache"
    );

    // Verify it exists
    let mut b = Batch::new();
    b.id(2);
    b.list_repos("repos", ddl::list_repos());
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();
    let repos = &resp.results["repos"].records[0]["repos"];
    assert!(repos.as_array().unwrap().contains(&json!("hot_cache")));
}

#[tokio::test]
async fn test_drop_repo() {
    let shamir = setup_shamir().await;

    // Create then drop
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("create", ddl::create_repo("temp").engine("in_memory"));
    b.drop_repo("drop", ddl::drop_repo("temp"));
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();
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
    exec_built(&shamir, to_req(&b)).await;

    // Create index
    let mut b = Batch::new();
    b.id(2);
    b.create_index(
        "idx",
        ddl::create_index("email_idx", "users")
            .fields(vec![vec!["email".to_owned()]])
            .unique(),
    );
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();
    assert_eq!(resp.results["idx"].records[0]["created_index"], "email_idx");
    assert_eq!(resp.results["idx"].records[0]["unique"], true);

    // Now query using the index
    let mut b = Batch::new();
    b.id(3);
    b.query(
        "find",
        Query::from("users").where_eq("email", "alice@test.com"),
    );
    let resp = exec_built(&shamir, to_req(&b)).await;
    assert_eq!(resp.results["find"].records.len(), 1);
    assert_eq!(resp.results["find"].records[0]["name"], "Alice");
}

#[tokio::test]
async fn test_drop_index_via_query() {
    let shamir = setup_shamir().await;

    // Create then drop index
    let mut b = Batch::new();
    b.id(1);
    b.create_index(
        "create",
        ddl::create_index("name_idx", "users").fields(vec![vec!["name".to_owned()]]),
    );
    b.drop_index("drop", ddl::drop_index("name_idx", "users"));
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();
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
    let mut b = Batch::new();
    b.id("setup");
    b.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["products", "orders"]),
    );
    shamir.execute("app", &to_req(&b)).await.unwrap();

    // Step 2: Insert data + create index (mixed admin + write)
    let mut b = Batch::new();
    b.id("populate");
    b.insert(
        "products",
        insert("products").rows([
            doc! { "name" => "Widget", "price" => 10 },
            doc! { "name" => "Gadget", "price" => 25 },
            doc! { "name" => "Widget Pro", "price" => 50 },
        ]),
    );
    b.create_index(
        "idx",
        ddl::create_index("price_idx", "products").fields(vec![vec!["price".to_owned()]]),
    );
    let resp = shamir.execute("app", &to_req(&b)).await.unwrap();
    assert_eq!(resp.results["products"].records.len(), 3);

    // Step 3: Query using index
    let mut b = Batch::new();
    b.id("query");
    b.query("cheap", Query::from("products").where_eq("price", 10));
    let resp = shamir.execute("app", &to_req(&b)).await.unwrap();
    assert_eq!(resp.results["cheap"].records.len(), 1);
    assert_eq!(resp.results["cheap"].records[0]["name"], "Widget");
}

// ============================================================================
// List indexes
// ============================================================================

#[tokio::test]
async fn test_list_indexes() {
    let shamir = setup_shamir().await;

    // Seed + create indexes (mixed admin + write)
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "seed",
        insert("users").rows([doc! { "name" => "Alice", "email" => "a@test.com" }]),
    );
    b.create_index(
        "idx1",
        ddl::create_index("name_idx", "users").fields(vec![vec!["name".to_owned()]]),
    );
    b.create_index(
        "idx2",
        ddl::create_index("email_idx", "users")
            .fields(vec![vec!["email".to_owned()]])
            .unique(),
    );
    shamir.execute("testdb", &to_req(&b)).await.unwrap();

    // List indexes
    let mut b = Batch::new();
    b.id(2);
    b.list_indexes("idxs", ddl::list_indexes("users"));
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();

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
// Create/drop table -- actually works
// ============================================================================

#[tokio::test]
async fn test_create_table_then_use_it() {
    let shamir = setup_shamir().await;

    // Create a new table via DDL
    let mut b = Batch::new();
    b.id(1);
    b.create_table("ct", ddl::create_table("products").repo("main"));
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();
    assert_eq!(resp.results["ct"].records[0]["created_table"], "products");

    // Verify it appears in list
    let mut b = Batch::new();
    b.id(2);
    b.list_tables("tables", ddl::list_tables().repo("main"));
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();
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
    let resp = exec_built(&shamir, to_req(&b)).await;
    assert_eq!(resp.results["ins"].records.len(), 2);

    // Read back
    let mut b = Batch::new();
    b.id(4);
    b.query("all", Query::from("products"));
    let resp = exec_built(&shamir, to_req(&b)).await;
    assert_eq!(resp.results["all"].records.len(), 2);
}

#[tokio::test]
async fn test_drop_table() {
    let shamir = setup_shamir().await;

    // Drop existing table
    let mut b = Batch::new();
    b.id(1);
    b.drop_table("dt", ddl::drop_table("users").repo("main"));
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();
    assert_eq!(resp.results["dt"].records[0]["existed"], true);

    // Verify it's gone -- insert should fail with table not found
    let mut b = Batch::new();
    b.id(2);
    b.insert("ins", insert("users").row(doc! { "name" => "Alice" }));
    let err = shamir.execute("testdb", &to_req(&b)).await.unwrap_err();
    assert!(matches!(
        err,
        shamir_db::query::batch::BatchError::QueryError { .. }
    ));
}

#[tokio::test]
async fn test_drop_nonexistent_table() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.drop_table("dt", ddl::drop_table("nonexistent").repo("main"));
    let resp = shamir.execute("testdb", &to_req(&b)).await.unwrap();
    assert_eq!(resp.results["dt"].records[0]["existed"], false);
}

// ============================================================================
// Error cases
// ============================================================================

#[tokio::test]
async fn test_admin_unknown_repo_error() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.list_tables("tables", ddl::list_tables().repo("nonexistent"));
    let err = shamir.execute("testdb", &to_req(&b)).await.unwrap_err();
    assert!(matches!(
        err,
        shamir_db::query::batch::BatchError::QueryError { .. }
    ));
}
