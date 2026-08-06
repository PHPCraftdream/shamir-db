//! P1-2 (#1015) — DDL Result Contract — tests for op_id minting and status polling.
//!
//! Tests cover:
//! - DROP/RENAME INDEX return QueryResult with op_id and ddl_status: Succeeded
//! - GetDdlOpStatus poll finds the correct record for the op_id
//! - Polling unknown op_id returns None
//! - Polling right op_id but wrong table returns None (routing correctness)

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::types::record_id::RecordId;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;

/// Setup in-memory ShamirDb with testdb/main/items table and a regular index.
async fn setup_with_index() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    // Create an index
    table.create_index("idx_city", &["city"]).await.unwrap();

    shamir
}

/// DROP INDEX returns QueryResult with op_id and ddl_status: Succeeded.
#[tokio::test]
async fn drop_index_returns_op_id_and_status() {
    let shamir = setup_with_index().await;

    let mut b = Batch::new();
    b.id(1);
    b.drop_index("d", ddl::drop_index("idx_city", "items").repo("main"));
    let req = b.to_request_via_msgpack();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    let result = &resp.results["d"];

    // op_id should be set
    assert!(
        result.op_id.is_some(),
        "DROP INDEX must return op_id in QueryResult"
    );

    // ddl_status should be Succeeded (immediate success for regular hash index)
    match &result.ddl_status {
        Some(shamir_query_types::read::DdlOpState::Succeeded { .. }) => {
            // Expected case
        }
        other => panic!(
            "DROP INDEX should have ddl_status: Succeeded, got {:?}",
            other
        ),
    }
}

/// RENAME INDEX returns QueryResult with op_id and ddl_status: Succeeded.
#[tokio::test]
async fn rename_index_returns_op_id_and_status() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    // Create an index to rename
    table.create_index("idx_city", &["city"]).await.unwrap();

    // Now rename it
    let mut b = Batch::new();
    b.id(1);
    b.rename_index(
        "r",
        ddl::rename_index("items", "idx_city", "idx_city_renamed").repo("main"),
    );
    let req = b.to_request_via_msgpack();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    let result = &resp.results["r"];

    // op_id should be set
    assert!(
        result.op_id.is_some(),
        "RENAME INDEX must return op_id in QueryResult"
    );

    // ddl_status should be Succeeded (immediate success for regular hash index)
    match &result.ddl_status {
        Some(shamir_query_types::read::DdlOpState::Succeeded { .. }) => {
            // Expected case
        }
        other => panic!(
            "RENAME INDEX should have ddl_status: Succeeded, got {:?}",
            other
        ),
    }
}

/// GetDdlOpStatus poll finds the correct record for the op_id.
#[tokio::test]
async fn poll_finds_correct_op_status_record() {
    let shamir = setup_with_index().await;

    // Execute DROP INDEX
    let mut b = Batch::new();
    b.id(1);
    b.drop_index("d", ddl::drop_index("idx_city", "items").repo("main"));
    let req = b.to_request_via_msgpack();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    let result = &resp.results["d"];
    let op_id = result.op_id.as_ref().expect("op_id must be set");

    // Poll the status
    let status = shamir
        .get_ddl_op_status("testdb", "main", "items", &op_id.to_string())
        .await
        .expect("get_ddl_op_status should succeed")
        .expect("status record should exist");

    assert_eq!(
        status.op_id, *op_id,
        "returned status should have matching op_id"
    );

    // Verify state is Succeeded (any timestamp is acceptable)
    match status.state {
        shamir_query_types::read::DdlOpState::Succeeded { .. } => {
            // Expected case
        }
        other => panic!("status should be Succeeded, got {:?}", other),
    }
}

/// Polling unknown op_id returns None.
#[tokio::test]
async fn poll_unknown_op_id_returns_none() {
    let shamir = setup_with_index().await;

    // Use a fake op_id (valid RecordId format but non-existent)
    let fake_op_id = RecordId::system("fake_unknown_op_id_never_exists").to_string();

    let status = shamir
        .get_ddl_op_status("testdb", "main", "items", &fake_op_id)
        .await
        .expect("get_ddl_op_status should succeed");

    assert!(status.is_none(), "polling unknown op_id should return None");
}

/// Polling right op_id but wrong table returns None (routing correctness).
#[tokio::test]
async fn poll_right_op_id_wrong_table_returns_none() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    // Create two tables in the same repo
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("items"))
        .add_table(TableConfig::new("orders"));
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let db = shamir.get_db("testdb").unwrap();
    let items_table = db.get_table("main", "items").await.unwrap();
    items_table
        .create_index("idx_items", &["city"])
        .await
        .unwrap();

    // Execute DROP INDEX on items table
    let mut b = Batch::new();
    b.id(1);
    b.drop_index("d", ddl::drop_index("idx_items", "items").repo("main"));
    let req = b.to_request_via_msgpack();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    let result = &resp.results["d"];
    let op_id = result.op_id.as_ref().expect("op_id must be set");

    // Poll for the same op_id but on the WRONG table (orders instead of items)
    let status = shamir
        .get_ddl_op_status("testdb", "main", "orders", &op_id.to_string())
        .await
        .expect("get_ddl_op_status should succeed");

    assert!(
        status.is_none(),
        "polling right op_id on wrong table should return None (routing correctness)"
    );

    // Confirm we CAN find it on the CORRECT table
    let correct_status = shamir
        .get_ddl_op_status("testdb", "main", "items", &op_id.to_string())
        .await
        .expect("get_ddl_op_status should succeed")
        .expect("status should be found on correct table");

    assert_eq!(
        correct_status.op_id, *op_id,
        "status should be found on the correct table"
    );
}
