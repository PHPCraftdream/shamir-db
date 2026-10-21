//! P1-2 (#1015) — DDL Result Contract — tests for op_id minting and status polling.
//!
//! Tests cover:
//! - DROP/RENAME INDEX return QueryResult with op_id and ddl_status: Succeeded
//! - GetDdlOpStatus poll finds the correct record for the op_id
//! - Polling unknown op_id returns None
//! - Polling right op_id but wrong table returns None (routing correctness)
//! - #1051: two DROP INDEX dispatches on one table mint distinct op_ids and
//!   are independently pollable — pins `RecordId::new()` at the actual
//!   dispatch site (`handle_drop_index`), not just at the lower engine
//!   layer `p1051_op_id_uniqueness_tests.rs` (shamir-engine) already covers.

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

/// #1051: two DROP INDEX ops on the SAME table, dispatched through the real
/// `handle_drop_index` entry point (not constructed directly), must mint
/// DISTINCT op_ids, and each must be independently pollable to the correct
/// operation. This is the regression pin the #1051 review (finding 2) asked
/// for: reverting `handle_drop_index`'s `RecordId::new()` mint back to the
/// old `RecordId::system(&format!("ddl_drop_index_{name}"))` formula makes
/// this test fail (both op_ids collapse to the same value) while every
/// other test in the workspace stays green, since they all construct their
/// own op_id directly rather than going through dispatch.
#[tokio::test]
async fn two_drop_index_dispatches_on_one_table_mint_distinct_pollable_op_ids() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();
    table.create_index("idx_city", &["city"]).await.unwrap();
    table.create_index("idx_zip", &["zip"]).await.unwrap();

    let mut b1 = Batch::new();
    b1.id(1);
    b1.drop_index("d", ddl::drop_index("idx_city", "items").repo("main"));
    let resp1 = shamir
        .execute("testdb", &b1.to_request_via_msgpack())
        .await
        .unwrap();
    let op_id_city = *resp1.results["d"]
        .op_id
        .as_ref()
        .expect("op_id must be set");

    let mut b2 = Batch::new();
    b2.id(1);
    b2.drop_index("d", ddl::drop_index("idx_zip", "items").repo("main"));
    let resp2 = shamir
        .execute("testdb", &b2.to_request_via_msgpack())
        .await
        .unwrap();
    let op_id_zip = *resp2.results["d"]
        .op_id
        .as_ref()
        .expect("op_id must be set");

    assert_ne!(
        op_id_city, op_id_zip,
        "two DROP INDEX ops on the same table must mint distinct op_ids"
    );

    let status_city = shamir
        .get_ddl_op_status("testdb", "main", "items", &op_id_city.to_string())
        .await
        .expect("get_ddl_op_status should succeed")
        .expect("idx_city's status record must exist");
    let status_zip = shamir
        .get_ddl_op_status("testdb", "main", "items", &op_id_zip.to_string())
        .await
        .expect("get_ddl_op_status should succeed")
        .expect("idx_zip's status record must exist");

    match &status_city.kind {
        shamir_query_types::read::DdlOpKind::DropHashIndex { index_name } => {
            assert_eq!(index_name, "idx_city")
        }
        other => panic!("expected DropHashIndex for idx_city, got {:?}", other),
    }
    match &status_zip.kind {
        shamir_query_types::read::DdlOpKind::DropHashIndex { index_name } => {
            assert_eq!(index_name, "idx_zip")
        }
        other => panic!("expected DropHashIndex for idx_zip, got {:?}", other),
    }
}
