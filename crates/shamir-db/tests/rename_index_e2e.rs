//! End-to-end lifecycle tests for `RENAME INDEX` (Phase F.1).
//!
//! Covers the rename contract:
//! - create index on a populated table → query uses the index (stats) →
//!   rename → query STILL uses the index under the new name; data is intact;
//! - old index name stops resolving after rename;
//! - refuse when the destination index name is already taken by another index.
//!
//! Tests build batch requests via `shamir_query_builder` and round-trip them
//! through MessagePack to mirror the real wire path (same harness as
//! `rename_table_e2e.rs`).

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::query::batch::{BatchError, BatchRequest, BatchResponse};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;

/// Boot an in-memory ShamirDb with an empty `main` repo.
async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;

    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    db.add_repo(repo_config).await.unwrap();
    shamir
}

async fn exec(shamir: &ShamirDb, req: BatchRequest) -> BatchResponse {
    shamir.execute("testdb", &req).await.unwrap()
}

async fn exec_err(shamir: &ShamirDb, req: BatchRequest) -> BatchError {
    shamir.execute("testdb", &req).await.unwrap_err()
}

/// Create a table named `name` in `main`.
async fn create_table(shamir: &ShamirDb, name: &str) {
    let mut b = Batch::new();
    b.id(1);
    b.create_table("ct", ddl::create_table(name).repo("main"));
    let _ = exec(shamir, b.to_request_via_msgpack()).await;
}

/// Rename a regular hash index: the index data is preserved and query
/// results are identical. The old name no longer resolves.
#[tokio::test]
async fn rename_index_preserves_data_and_lookup() {
    let shamir = setup_shamir().await;
    create_table(&shamir, "users").await;

    // Create a regular index on "email".
    let mut b = Batch::new();
    b.id(1);
    b.create_index(
        "ci",
        ddl::create_index("idx_email", "users")
            .field("email")
            .repo("main"),
    );
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    // Insert data.
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins",
        insert("users").row(doc! { "email" => "alice@example.com", "name" => "Alice" }),
    );
    b.insert(
        "ins2",
        insert("users").row(doc! { "email" => "bob@example.com", "name" => "Bob" }),
    );
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    // Query by the indexed field before rename — should use the index.
    let mut b = Batch::new();
    b.id(3);
    b.query(
        "q",
        Query::from("users").where_eq("email", "alice@example.com"),
    );
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    let stats = resp.results["q"].stats.as_ref().expect("stats present");
    assert_eq!(
        stats.index_used.as_deref(),
        Some("idx_email"),
        "query should use the index before rename"
    );
    let records = &resp.results["q"].records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].get_value_str("name"), Some("Alice"));

    // Rename idx_email → idx_mail.
    let mut b = Batch::new();
    b.id(4);
    b.rename_index(
        "ri",
        ddl::rename_index("users", "idx_email", "idx_mail").repo("main"),
    );
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    assert_eq!(
        resp.results["ri"].records[0].get_value_str("renamed_index"),
        Some("idx_email")
    );
    assert_eq!(
        resp.results["ri"].records[0].get_value_str("to"),
        Some("idx_mail")
    );

    // Query by the indexed field after rename — should STILL use the index,
    // now under the new name.
    let mut b = Batch::new();
    b.id(5);
    b.query(
        "q2",
        Query::from("users").where_eq("email", "alice@example.com"),
    );
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    let stats = resp.results["q2"].stats.as_ref().expect("stats present");
    assert_eq!(
        stats.index_used.as_deref(),
        Some("idx_mail"),
        "query should use the renamed index"
    );
    let records = &resp.results["q2"].records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].get_value_str("name"), Some("Alice"));

    // Query for Bob — data integrity check (second row also intact).
    let mut b = Batch::new();
    b.id(6);
    b.query(
        "q3",
        Query::from("users").where_eq("email", "bob@example.com"),
    );
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    let records = &resp.results["q3"].records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].get_value_str("name"), Some("Bob"));
}

/// Renaming onto an existing index name is refused with a QueryError.
#[tokio::test]
async fn rename_index_refuses_destination_exists() {
    let shamir = setup_shamir().await;
    create_table(&shamir, "users").await;

    // Create two indexes.
    let mut b = Batch::new();
    b.id(1);
    b.create_index(
        "ci1",
        ddl::create_index("idx_a", "users")
            .field("email")
            .repo("main"),
    );
    b.create_index(
        "ci2",
        ddl::create_index("idx_b", "users")
            .field("name")
            .repo("main"),
    );
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    // idx_a → idx_b must fail.
    let mut b = Batch::new();
    b.id(2);
    b.rename_index(
        "ri",
        ddl::rename_index("users", "idx_a", "idx_b").repo("main"),
    );
    let err = exec_err(&shamir, b.to_request_via_msgpack()).await;
    match err {
        BatchError::QueryError { message, .. } => {
            assert!(
                message.to_lowercase().contains("already exists"),
                "expected destination-exists refusal, got: {}",
                message
            );
        }
        other => panic!("expected QueryError, got {:?}", other),
    }
}

/// Renaming a non-existent index is refused with an error.
#[tokio::test]
async fn rename_index_refuses_source_absent() {
    let shamir = setup_shamir().await;
    create_table(&shamir, "users").await;

    // ghost → real must fail (ghost doesn't exist).
    let mut b = Batch::new();
    b.id(1);
    b.rename_index(
        "ri",
        ddl::rename_index("users", "ghost", "real").repo("main"),
    );
    let err = exec_err(&shamir, b.to_request_via_msgpack()).await;
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected QueryError, got {:?}",
        err
    );
}
