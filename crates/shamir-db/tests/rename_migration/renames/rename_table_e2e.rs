//! End-to-end lifecycle tests for `RENAME TABLE` (Phase E.4 #244 — Object 1).
//!
//! Covers the rename contract documented in `docs/dev-artifacts/prompts/ddl-lifecycle/04-rename.md`:
//! - rename an empty table: old name stops resolving, new name resolves and
//!   accepts/serves data, the catalogue reflects the new name only;
//! - rename a populated table (Phase F.2): the MVCC overlay is force-drained
//!   into `__history__` before the store copy, so all committed rows travel
//!   with the renamed table;
//! - refuse when the destination already exists;
//! - refuse when the table carries a declarative schema (auto-bound schema
//!   validator embeds the table path — migrating it is a follow-on).
//!
//! Tests build batch requests via `shamir_query_builder` and round-trip them
//! through MessagePack to mirror the real wire path (same harness as
//! `query_admin.rs`).

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::query::batch::{BatchError, BatchRequest, BatchResponse};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;

/// Boot an in-memory ShamirDb with an empty `main` repo. Tables are
/// created via DDL inside each test so the catalogue row is written
/// (rename reads the persisted record to preserve ResourceMeta and to
/// guard against schema / FK references).
async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;

    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Create a table named `name` in `main` via the DDL path (writes the
/// catalogue row that `rename_table_as` reads).
async fn create_table(shamir: &ShamirDb, name: &str) {
    let mut b = Batch::new();
    b.id(1);
    b.create_table("ct", ddl::create_table(name).repo("main"));
    let _ = exec(shamir, b.to_request_via_msgpack()).await;
}

async fn exec(shamir: &ShamirDb, req: BatchRequest) -> BatchResponse {
    shamir.execute("testdb", &req).await.unwrap()
}

async fn exec_err(shamir: &ShamirDb, req: BatchRequest) -> BatchError {
    shamir.execute("testdb", &req).await.unwrap_err()
}

/// Rename an empty (just-created) table: the catalogue row, in-memory
/// config, and reverse-index entry are all migrated. Old name stops
/// resolving, new name resolves.
#[tokio::test]
async fn rename_table_migrates_empty_table() {
    let shamir = setup_shamir().await;
    create_table(&shamir, "users").await;

    // Rename users → people.
    let mut b = Batch::new();
    b.id(1);
    b.rename_table("rn", ddl::rename_table("users", "people").repo("main"));
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    assert_eq!(
        resp.results["rn"].records[0].get_value_str("renamed_table"),
        Some("users")
    );
    assert_eq!(
        resp.results["rn"].records[0].get_value_str("to"),
        Some("people")
    );

    // Old name is gone: insert into `users` must fail.
    let mut b = Batch::new();
    b.id(2);
    b.insert("ins", insert("users").row(doc! { "name" => "Carol" }));
    let err = exec_err(&shamir, b.to_request_via_msgpack()).await;
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected QueryError, got {:?}",
        err
    );

    // New name resolves — a fresh row can be inserted and read back.
    let mut b = Batch::new();
    b.id(3);
    b.insert("ins", insert("people").row(doc! { "name" => "Dave" }));
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(4);
    b.query("all", Query::from("people"));
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    let records = &resp.results["all"].records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].get_value_str("name"), Some("Dave"));

    // Catalogue reflects the new name only.
    let mut b = Batch::new();
    b.id(5);
    b.list_tables("tables", ddl::list_tables().repo("main"));
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    let rec = resp.results["tables"].records[0].as_value().into_owned();
    let tables = rec["tables"].as_array().unwrap();
    assert!(tables.iter().any(|v| v == "people"));
    assert!(!tables.iter().any(|v| v == "users"));
}

/// Renaming onto an existing table name is refused with a QueryError.
#[tokio::test]
async fn rename_table_refuses_destination_exists() {
    let shamir = setup_shamir().await;
    create_table(&shamir, "users").await;

    // Create a second table `other`.
    let mut b = Batch::new();
    b.id(1);
    b.create_table("ct", ddl::create_table("other").repo("main"));
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    // users → other must fail.
    let mut b = Batch::new();
    b.id(2);
    b.rename_table("rn", ddl::rename_table("users", "other").repo("main"));
    let err = exec_err(&shamir, b.to_request_via_msgpack()).await;
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected QueryError, got {:?}",
        err
    );
}

/// A table that carries a declarative schema cannot be renamed yet —
/// the auto-bound schema validator is registered under a name that
/// embeds the table path, so a rename would orphan it. The guard
/// refuses up-front instead.
#[tokio::test]
async fn rename_table_refuses_schema_bearing() {
    let shamir = setup_shamir().await;
    create_table(&shamir, "users").await;

    // Attach a declarative schema to `users`.
    let mut b = Batch::new();
    b.id(1);
    b.set_table_schema(
        "sc",
        ddl::set_table_schema("users")
            .repo("main")
            .rules(vec![ddl::field(["name"]).string().required().build()]),
    );
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    // users → accounts must fail with a QueryError mentioning schema.
    let mut b = Batch::new();
    b.id(2);
    b.rename_table("rn", ddl::rename_table("users", "accounts").repo("main"));
    let err = exec_err(&shamir, b.to_request_via_msgpack()).await;
    match err {
        BatchError::QueryError { message, .. } => {
            assert!(
                message.to_lowercase().contains("schema"),
                "expected schema-related refusal, got: {}",
                message
            );
        }
        other => panic!("expected QueryError, got {:?}", other),
    }
}

/// Rename a **populated** table: the MVCC overlay is force-drained into
/// `__history__` before the store copy (Phase F.2), so ALL committed rows
/// travel with the renamed table. After rename:
/// - the old name no longer resolves;
/// - the new name resolves with every previously-inserted row;
/// - indexes on the new table work;
/// - a new row can be inserted into the renamed table and read back
///   (the new MvccStore's overlay is live).
#[tokio::test]
async fn rename_table_migrates_populated() {
    let shamir = setup_shamir().await;
    create_table(&shamir, "users").await;

    // Insert 3 rows so the MVCC overlay is non-empty.
    for name in ["Alice", "Bob", "Carol"] {
        let mut b = Batch::new();
        b.id(1);
        b.insert("ins", insert("users").row(doc! { "name" => name }));
        let _ = exec(&shamir, b.to_request_via_msgpack()).await;
    }

    // Rename users → people (must succeed — F.2 force-drains the overlay).
    let mut b = Batch::new();
    b.id(2);
    b.rename_table("rn", ddl::rename_table("users", "people").repo("main"));
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    assert_eq!(
        resp.results["rn"].records[0].get_value_str("renamed_table"),
        Some("users")
    );

    // Old name no longer resolves — insert into `users` must fail.
    let mut b = Batch::new();
    b.id(3);
    b.insert("ins", insert("users").row(doc! { "name" => "Ghost" }));
    let err = exec_err(&shamir, b.to_request_via_msgpack()).await;
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected QueryError for old name, got {:?}",
        err
    );

    // New name resolves — query ALL rows back (data migrated via drain).
    let mut b = Batch::new();
    b.id(4);
    b.query("all", Query::from("people"));
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    let records = &resp.results["all"].records;
    assert_eq!(
        records.len(),
        3,
        "renamed table must have all 3 migrated rows"
    );
    let names: shamir_collections::TFxSet<_> = records
        .iter()
        .filter_map(|r| r.get_value_str("name").map(|s| s.to_string()))
        .collect();
    assert!(names.contains("Alice"), "Alice must be in renamed table");
    assert!(names.contains("Bob"), "Bob must be in renamed table");
    assert!(names.contains("Carol"), "Carol must be in renamed table");

    // Append a new row into the renamed table — new MvccStore overlay is live.
    let mut b = Batch::new();
    b.id(5);
    b.insert("ins", insert("people").row(doc! { "name" => "Dave" }));
    let _ = exec(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(6);
    b.query("all2", Query::from("people"));
    let resp = exec(&shamir, b.to_request_via_msgpack()).await;
    let records = &resp.results["all2"].records;
    assert_eq!(
        records.len(),
        4,
        "renamed table must have 4 rows after append"
    );
    assert!(
        records
            .iter()
            .any(|r| r.get_value_str("name") == Some("Dave")),
        "appended row must be readable"
    );
}
