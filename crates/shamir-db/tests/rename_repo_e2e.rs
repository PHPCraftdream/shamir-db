//! End-to-end lifecycle tests for `RENAME REPO` (Phase F.3).
//!
//! Covers the rename contract documented in
//! `docs/dev-artifacts/prompts/ddl-lifecycle/07-rename-repo.md`:
//! - rename a repo that has a table + data + index: the old repo name
//!   stops resolving, the new name resolves, and ALL table data + the
//!   index remain intact and queryable under the new repo name;
//! - refuse when the destination repo already exists.
//!
//! The repo's table stores are keyed only by table name *inside* the repo
//! (the repo name is NOT part of the physical store namespace), so a rename
//! is a pure logical re-key — no `rename_table_stores` and no drain are
//! needed; the tables travel with the repo under the new key for free.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::query::batch::{BatchError, BatchRequest, BatchResponse};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::Insert;
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

/// Rename a repository that contains a populated table with a secondary
/// index. After rename:
/// - the old repo name no longer resolves (insert fails);
/// - the new repo name resolves and serves ALL previously-inserted rows;
/// - the index still works under the new repo name (lookup returns the
///   expected record id);
/// - a new row inserted under the new repo name is readable.
#[tokio::test]
async fn rename_repo_preserves_table_data_and_index() {
    let shamir = setup_shamir().await;

    // Create a second repo `analytics` with one table `events`.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("analytics"));
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(2);
    b.create_table("ct", ddl::create_table("events").repo("analytics"));
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    // Insert 3 rows so the table is populated.
    for name in ["Alice", "Bob", "Carol"] {
        let mut b = Batch::new();
        b.id(3);
        b.insert(
            "ins",
            Insert::with_repo("analytics", "events").row(doc! { "name" => name }),
        );
        let _ = exec(&shamir, b.to_request_via_msgpack()).await;
    }

    // Build a secondary index on `name` under analytics/events.
    let mut b = Batch::new();
    b.id(4);
    b.create_index(
        "ix",
        ddl::create_index("idx_name", "events")
            .repo("analytics")
            .field("name"),
    );
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    // Rename analytics → telemetry.
    let mut b = Batch::new();
    b.id(5);
    b.rename_repo("rn", ddl::rename_repo("analytics", "telemetry"));
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    assert_eq!(
        resp.results["rn"].records[0].get_value_str("renamed_repo"),
        Some("analytics")
    );
    assert_eq!(
        resp.results["rn"].records[0].get_value_str("to"),
        Some("telemetry")
    );

    // Old repo name no longer resolves — insert under `analytics` must fail.
    let mut b = Batch::new();
    b.id(6);
    b.insert(
        "ins",
        Insert::with_repo("analytics", "events").row(doc! { "name" => "Ghost" }),
    );
    let err = exec_err(&shamir, b.to_request_via_msgpack()).await;
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected QueryError for old repo name, got {:?}",
        err
    );

    // New repo name resolves — query ALL rows back.
    let mut b = Batch::new();
    b.id(7);
    b.query("all", Query::with_repo("telemetry", "events"));
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    let records = &resp.results["all"].records;
    assert_eq!(
        records.len(),
        3,
        "renamed repo's table must have all 3 migrated rows"
    );
    let names: shamir_collections::TFxSet<_> = records
        .iter()
        .filter_map(|r| r.get_value_str("name").map(|s| s.to_string()))
        .collect();
    assert!(names.contains("Alice"), "Alice must be present");
    assert!(names.contains("Bob"), "Bob must be present");
    assert!(names.contains("Carol"), "Carol must be present");

    // The index still exists under the new repo name — list_indexes
    // must still show `idx_name` under telemetry/events.
    let mut b = Batch::new();
    b.id(8);
    b.list_indexes("ixs", ddl::list_indexes("events").repo("telemetry"));
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    let rec = resp.results["ixs"]
        .records
        .first()
        .expect("list_indexes must return a record")
        .as_value()
        .into_owned();
    let index_names_json = rec["indexes"].as_array().expect("indexes array");
    // list_indexes returns an array of objects `{name, unique}`, not
    // bare strings — extract the `name` field from each entry.
    let found = index_names_json
        .iter()
        .any(|v| v["name"].as_str().is_some_and(|s| s == "idx_name"));
    assert!(
        found,
        "idx_name must still be listed under telemetry/events after rename; got {:?}",
        index_names_json
    );

    // Append a new row under the new repo name — must be readable.
    let mut b = Batch::new();
    b.id(10);
    b.insert(
        "ins",
        Insert::with_repo("telemetry", "events").row(doc! { "name" => "Dave" }),
    );
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(11);
    b.query("final", Query::with_repo("telemetry", "events"));
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    let records = &resp.results["final"].records;
    assert_eq!(
        records.len(),
        4,
        "table under renamed repo must have 4 rows after append"
    );
    assert!(
        records
            .iter()
            .any(|r| r.get_value_str("name") == Some("Dave")),
        "appended row must be readable under the new repo name"
    );
}

/// Renaming onto an existing repo name is refused with a QueryError.
#[tokio::test]
async fn rename_repo_refuses_destination_exists() {
    let shamir = setup_shamir().await;

    // Create a second repo `target`.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("target"));
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    // main → target must fail.
    let mut b = Batch::new();
    b.id(2);
    b.rename_repo("rn", ddl::rename_repo("main", "target"));
    let err = exec_err(&shamir, b.to_request_via_msgpack()).await;
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected QueryError for destination-exists, got {:?}",
        err
    );
}

/// Renaming a non-existent source repo is refused (NotFound → QueryError).
#[tokio::test]
async fn rename_repo_refuses_source_absent() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.rename_repo("rn", ddl::rename_repo("ghost", "phantom"));
    let err = exec_err(&shamir, b.to_request_via_msgpack()).await;
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected QueryError for source-absent, got {:?}",
        err
    );
}
