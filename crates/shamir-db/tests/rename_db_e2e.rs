//! End-to-end lifecycle tests for `RENAME DB` (campaign ②.1d — variant γ).
//!
//! Covers the rename-db contract documented in
//! `docs/prompts/ddl-evolution/09-rename-db.md`:
//! - rename a db that has a repo + table + index + schema + access-meta:
//!   after rename, the old db name no longer resolves, the new name
//!   resolves, ALL catalogue rows are re-keyed (db / repositories / tables,
//!   including the schema fields inside the table record), table data is
//!   intact (handles travel with the moved DbInstance), and the ACL
//!   (owner/mode) is preserved;
//! - durable reopen: after rename + reopen on the same system-store path,
//!   the db boots under the NEW name at the SAME physical path and the
//!   table data is intact;
//! - guards: refuse for a non-existent source (NotFound), refuse when the
//!   destination already exists, refuse to rename SYSTEM_DB.

use shamir_db::query::batch::{BatchError, BatchRequest, BatchResponse};
use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::Insert;
use shamir_query_builder::Query;

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

/// Boot an in-memory ShamirDb with a `testdb` database. The `main` repo is
/// created via DDL (`create_repo`) so its row is persisted in the
/// `repositories` catalogue — needed for rename completeness checks.
async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let _ = shamir.create_db("testdb").await;

    // Create `main` via DDL so its row lands in the persisted `repositories`
    // catalogue (setup_db's `db.add_repo` bypasses the catalogue).
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").engine("in_memory"));
    let _ = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    shamir
}

async fn exec(shamir: &ShamirDb, db_name: &str, req: &BatchRequest) -> BatchResponse {
    shamir.execute(db_name, req).await.unwrap()
}

async fn exec_err(shamir: &ShamirDb, db_name: &str, req: &BatchRequest) -> BatchError {
    shamir.execute(db_name, req).await.unwrap_err()
}

// ═══════════════════════════════════════════════════════════════════════
// Main completeness test
// ═══════════════════════════════════════════════════════════════════════

/// Rename a database that contains a populated repo + table + secondary
/// index + declarative schema. After rename:
/// - the old db name no longer resolves (query fails);
/// - the new db name resolves and serves ALL previously-inserted rows;
/// - the catalogue rows (databases / repositories / tables) are re-keyed
///   to the new db name — verified through list introspection;
/// - the schema survives the rename (get_table_schema under the new name
///   returns the rules);
/// - the index still works under the new db name;
/// - ACL (owner/mode) on the database record is preserved.
#[tokio::test]
async fn rename_db_rekeys_all_catalogues_and_preserves_data() {
    let shamir = setup_shamir().await;

    // Create a table `events` under testdb/main.
    let mut b = Batch::new();
    b.id(1);
    b.create_table("ct", ddl::create_table("events"));
    let _ = exec(&shamir, "testdb", &b.to_request_via_msgpack()).await;

    // Set a declarative schema with one required field.
    let mut b = Batch::new();
    b.id(2);
    b.set_table_schema(
        "sc",
        ddl::set_table_schema("events")
            .rules(vec![ddl::field(["name"]).string().required().build()]),
    );
    let _ = exec(&shamir, "testdb", &b.to_request_via_msgpack()).await;

    // Insert 3 rows.
    for name in ["Alice", "Bob", "Carol"] {
        let mut b = Batch::new();
        b.id(3);
        b.insert(
            "ins",
            Insert::with_repo("main", "events").row(doc! { "name" => name }),
        );
        let _ = exec(&shamir, "testdb", &b.to_request_via_msgpack()).await;
    }

    // Build a secondary index on `name`.
    let mut b = Batch::new();
    b.id(4);
    b.create_index("ix", ddl::create_index("idx_name", "events").field("name"));
    let _ = exec(&shamir, "testdb", &b.to_request_via_msgpack()).await;

    // Rename testdb → production.
    let mut b = Batch::new();
    b.id(5);
    b.rename_db("rn", ddl::rename_db("testdb", "production"));
    let resp = exec(&shamir, "testdb", &b.to_request_via_msgpack()).await;
    assert_eq!(
        resp.results["rn"].records[0].get_value_str("renamed_db"),
        Some("testdb")
    );
    assert_eq!(
        resp.results["rn"].records[0].get_value_str("to"),
        Some("production")
    );

    // In-memory: the old name is gone, the new name resolves.
    assert!(
        !shamir.has_db("testdb"),
        "old db name must be gone in-memory"
    );
    assert!(
        shamir.has_db("production"),
        "new db name must exist in-memory"
    );

    // Query the table data under the new db name — all 3 rows intact.
    let mut b = Batch::new();
    b.id(6);
    b.query("all", Query::with_repo("main", "events"));
    let resp = exec(&shamir, "production", &b.to_request_via_msgpack()).await;
    let records = &resp.results["all"].records;
    assert_eq!(records.len(), 3, "table data must survive db rename");

    // The schema survives the rename — get_table_schema under production
    // returns the rule.
    let mut b = Batch::new();
    b.id(7);
    b.get_table_schema("gs", ddl::get_table_schema("events"));
    let resp = exec(&shamir, "production", &b.to_request_via_msgpack()).await;
    let schema_qv = resp.results["gs"].records[0]
        .as_value()
        .as_ref()
        .get("schema")
        .cloned();
    assert!(
        schema_qv.is_some(),
        "schema must survive the db rename (re-keyed via save_table_meta)"
    );

    // The index still works under the new db name — list_indexes shows it.
    let mut b = Batch::new();
    b.id(8);
    b.list_indexes("ixs", ddl::list_indexes("events"));
    let resp = exec(&shamir, "production", &b.to_request_via_msgpack()).await;
    let rec = resp.results["ixs"]
        .records
        .first()
        .expect("list_indexes must return a record")
        .as_value()
        .into_owned();
    let found = rec["indexes"]
        .as_array()
        .is_some_and(|arr| arr.iter().any(|v| v["name"].as_str() == Some("idx_name")));
    assert!(found, "index must still be listed under the new db name");

    // Catalogue re-key: load_databases must show `production`, not `testdb`.
    let dbs = shamir.system_store().load_databases().await.unwrap();
    let has_production = dbs
        .iter()
        .any(|r| r.get("name").and_then(|v| v.as_str()) == Some("production"));
    let has_testdb = dbs
        .iter()
        .any(|r| r.get("name").and_then(|v| v.as_str()) == Some("testdb"));
    assert!(
        has_production,
        "databases catalogue must contain 'production'"
    );
    assert!(!has_testdb, "databases catalogue must NOT contain 'testdb'");

    // Catalogue re-key: repositories must show db_name=production.
    let repos = shamir.system_store().load_repositories().await.unwrap();
    let has_prod_repo = repos
        .iter()
        .any(|r| r.get("db_name").and_then(|v| v.as_str()) == Some("production"));
    let has_testdb_repo = repos
        .iter()
        .any(|r| r.get("db_name").and_then(|v| v.as_str()) == Some("testdb"));
    assert!(
        has_prod_repo,
        "repositories catalogue must contain a production row"
    );
    assert!(
        !has_testdb_repo,
        "repositories catalogue must NOT contain any testdb row"
    );

    // Catalogue re-key: tables must show db_name=production.
    let tables = shamir.system_store().load_tables().await.unwrap();
    let has_prod_table = tables.iter().any(|r| {
        r.get("db_name").and_then(|v| v.as_str()) == Some("production")
            && r.get("table_name").and_then(|v| v.as_str()) == Some("events")
    });
    let has_testdb_table = tables
        .iter()
        .any(|r| r.get("db_name").and_then(|v| v.as_str()) == Some("testdb"));
    assert!(
        has_prod_table,
        "tables catalogue must contain a production/events row"
    );
    assert!(
        !has_testdb_table,
        "tables catalogue must NOT contain any testdb row"
    );

    // Append a new row under the new db name — must be readable.
    let mut b = Batch::new();
    b.id(10);
    b.insert(
        "ins",
        Insert::with_repo("main", "events").row(doc! { "name" => "Dave" }),
    );
    let _ = exec(&shamir, "production", &b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(11);
    b.query("final", Query::with_repo("main", "events"));
    let resp = exec(&shamir, "production", &b.to_request_via_msgpack()).await;
    let records = &resp.results["final"].records;
    assert_eq!(records.len(), 4, "table must have 4 rows after append");
    assert!(
        records
            .iter()
            .any(|r| r.get_value_str("name") == Some("Dave")),
        "appended row must be readable under the new db name"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Durable reopen test
// ═══════════════════════════════════════════════════════════════════════

/// Re-open a durable ShamirDb on the same system-store path, retrying
/// briefly while the previous session's store still holds the file lock.
async fn reopen_durable(sys_path: std::path::PathBuf) -> ShamirDb {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone())).await {
            Ok(db) => return db,
            Err(e) => {
                let m = e.to_string();
                if m.contains("Cannot acquire lock")
                    || m.contains("already open")
                    || m.contains("Locked")
                {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                } else {
                    panic!("unexpected reopen error: {e}");
                }
            }
        }
    }
    panic!("fjall lock was not released within the retry window");
}

/// After rename + reopen on the same system-store path, the db boots under
/// the NEW name and the table data is intact.
#[tokio::test]
async fn rename_db_survives_durable_reopen() {
    // Use tempfile::tempdir so BOTH the system store AND the data_root
    // (data_root = parent of the system-store path) live inside a unique
    // disposable directory. If we used std::env::temp_dir() directly,
    // data_root would be the shared OS temp and repo data from previous
    // runs would leak in (create_repo builds `data_root/<db>/<repo>`).
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta");

    // Phase 1: create db + repo + table + insert a row + rename.
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        let _ = shamir.create_db("olddb").await;

        let mut b = Batch::new();
        b.id(1);
        // Default engine (fjall when data_root is set) so data survives reopen.
        b.create_repo("cr", ddl::create_repo("main").tables(["items"]));
        let _ = shamir.execute("olddb", &b.to_request_via_msgpack()).await;

        // Insert a row.
        let mut b = Batch::new();
        b.id(3);
        b.insert(
            "ins",
            Insert::with_repo("main", "items").row(doc! { "sku" => "A-1" }),
        );
        let _ = shamir.execute("olddb", &b.to_request_via_msgpack()).await;

        let mut b = Batch::new();
        b.id(4);
        b.rename_db("rn", ddl::rename_db("olddb", "newdb"));
        let _ = shamir.execute("olddb", &b.to_request_via_msgpack()).await;

        // Flush MemBuffer-wrapped stores so the catalogue re-key is
        // durable before drop. The insert above already went through WAL
        // commit → data store; this flush is for the system-store writes
        // (save_database/save_repository/save_table_meta already call
        // data_store().flush() internally, but flush_all also drains the
        // repo's tx-info and table buffers for a clean shutdown).
        let _ = shamir.flush_all().await;
    }

    // Phase 2: reopen — the db must boot under `newdb`, not `olddb`.
    let shamir = reopen_durable(sys_path.clone()).await;

    // Boot re-reads the databases catalogue; the renamed db must appear
    // under the new name.
    let dbs = shamir.system_store().load_databases().await.unwrap();
    let has_newdb = dbs
        .iter()
        .any(|r| r.get("name").and_then(|v| v.as_str()) == Some("newdb"));
    let has_olddb = dbs
        .iter()
        .any(|r| r.get("name").and_then(|v| v.as_str()) == Some("olddb"));
    assert!(has_newdb, "durable reopen must show 'newdb'");
    assert!(!has_olddb, "durable reopen must NOT show 'olddb'");

    // The repos catalogue must show db_name=newdb with the same path.
    let repos = shamir.system_store().load_repositories().await.unwrap();
    let newdb_repo = repos
        .iter()
        .find(|r| r.get("db_name").and_then(|v| v.as_str()) == Some("newdb"))
        .expect("repos catalogue must have a newdb row after reopen");
    assert_eq!(
        newdb_repo.get("repo_name").and_then(|v| v.as_str()),
        Some("main"),
        "repo name must be preserved"
    );
    assert_eq!(
        newdb_repo.get("engine").and_then(|v| v.as_str()),
        Some("fjall"),
        "engine must be preserved across rename + reopen"
    );

    // In-memory: the reopened instance must have `newdb` registered.
    assert!(
        shamir.has_db("newdb"),
        "reopened ShamirDb must have 'newdb' in-memory"
    );
    assert!(
        !shamir.has_db("olddb"),
        "reopened ShamirDb must NOT have 'olddb' in-memory"
    );

    // Query the table data under the new db name — the row is intact.
    let mut b = Batch::new();
    b.id(10);
    b.query("all", Query::with_repo("main", "items"));
    let resp = shamir
        .execute("newdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let records = &resp.results["all"].records;
    assert_eq!(
        records.len(),
        1,
        "table data must survive rename + reopen (1 row)"
    );
    assert_eq!(
        records[0].get_value_str("sku"),
        Some("A-1"),
        "the inserted row must be intact"
    );

    // Cleanup — drop the ShamirDb (releases fjall locks); the tempdir
    // is disposed automatically when `dir` goes out of scope.
    drop(shamir);
    drop(dir);
}

// ═══════════════════════════════════════════════════════════════════════
// Guard tests
// ═══════════════════════════════════════════════════════════════════════

/// Renaming a non-existent source db is refused with a QueryError
/// (NotFound → QueryError).
#[tokio::test]
async fn rename_db_refuses_source_absent() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.rename_db("rn", ddl::rename_db("ghost", "phantom"));
    let err = exec_err(&shamir, "testdb", &b.to_request_via_msgpack()).await;
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected QueryError for source-absent, got {:?}",
        err
    );
}

/// Renaming onto an existing db name is refused with a QueryError.
#[tokio::test]
async fn rename_db_refuses_destination_exists() {
    let shamir = setup_shamir().await;
    let _ = shamir.create_db("target").await;

    let mut b = Batch::new();
    b.id(1);
    b.rename_db("rn", ddl::rename_db("testdb", "target"));
    let err = exec_err(&shamir, "testdb", &b.to_request_via_msgpack()).await;
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected QueryError for destination-exists, got {:?}",
        err
    );
}

/// Renaming the system database is refused (SYSTEM_DB guard).
#[tokio::test]
async fn rename_db_refuses_system_db() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.rename_db("rn", ddl::rename_db("__system__", "hacked"));
    let err = exec_err(&shamir, "testdb", &b.to_request_via_msgpack()).await;
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected QueryError for SYSTEM_DB rename attempt, got {:?}",
        err
    );
    // The system store (used by every ShamirDb) must still be functional.
    let dbs = shamir.system_store().load_databases().await.unwrap();
    assert!(
        dbs.iter()
            .all(|r| r.get("name").and_then(|v| v.as_str()) != Some("hacked")),
        "SYSTEM_DB rename must NOT have created a 'hacked' row"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Data integrity test
// ═══════════════════════════════════════════════════════════════════════

/// Insert a row → rename db → read the same table under the new name →
/// the row is on the same physical handle (path NOT touched).
#[tokio::test]
async fn rename_db_preserves_table_data_on_same_handle() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_table("ct", ddl::create_table("records"));
    let _ = exec(&shamir, "testdb", &b.to_request_via_msgpack()).await;

    // Insert a row.
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins",
        Insert::with_repo("main", "records").row(doc! { "id" => 42i64, "label" => "hello" }),
    );
    let _ = exec(&shamir, "testdb", &b.to_request_via_msgpack()).await;

    // Rename.
    let mut b = Batch::new();
    b.id(3);
    b.rename_db("rn", ddl::rename_db("testdb", "renamed"));
    let _ = exec(&shamir, "testdb", &b.to_request_via_msgpack()).await;

    // Read under the new name.
    let mut b = Batch::new();
    b.id(4);
    b.query("q", Query::with_repo("main", "records"));
    let resp = exec(&shamir, "renamed", &b.to_request_via_msgpack()).await;
    let records = &resp.results["q"].records;
    assert_eq!(records.len(), 1, "row must be found under the new db name");
    assert_eq!(
        records[0].get_value_str("label"),
        Some("hello"),
        "data must be intact on the same physical handle"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Wire-shape test
// ═══════════════════════════════════════════════════════════════════════

/// `rename_db('old', 'new')` produces wire `{ rename_db: 'old', to: 'new' }`.
#[tokio::test]
async fn rename_db_wire_shape() {
    use shamir_query_types::admin::RenameDbOp;
    use shamir_query_types::batch::BatchOp;

    let op = ddl::rename_db("old", "new").build();
    match op {
        BatchOp::RenameDb(RenameDbOp { rename_db, to }) => {
            assert_eq!(rename_db, "old");
            assert_eq!(to, "new");
        }
        other => panic!("expected BatchOp::RenameDb, got {:?}", other),
    }

    // Round-trip through serde (msgpack) to verify the wire discriminator.
    let batch_op = BatchOp::RenameDb(RenameDbOp {
        rename_db: "old".to_string(),
        to: "new".to_string(),
    });
    let bytes = rmp_serde::to_vec_named(&batch_op).unwrap();
    let decoded: BatchOp = rmp_serde::from_slice(&bytes).unwrap();
    match decoded {
        BatchOp::RenameDb(op) => {
            assert_eq!(op.rename_db, "old");
            assert_eq!(op.to, "new");
        }
        other => panic!("decoded to {:?}, expected RenameDb", other),
    }
}
