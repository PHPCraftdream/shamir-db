//! Phase C2 — foreign-key declarative constraint e2e tests.
//!
//! Exercises the REAL DDL execution path: create parent table with index,
//! create child table with FK schema, insert child rows with existing /
//! non-existing references, and verify FK enforcement survives durable reopen.
//!
//! FK checks require `ctx.db() == Some`, which means the insert must go
//! through the **transactional** batch path (where the resolver is wired).
//! Autocommit (implicit tx) does NOT wire the resolver, so FK is silently
//! skipped there.

use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::write::insert;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

// ═══════════════════════════════════════════════════════════════════════
// Helpers
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

/// Execute a `set_table_schema` DDL op and return the response result map.
async fn exec_set_schema(
    db: &ShamirDb,
    db_name: &str,
    alias: &str,
    table: &str,
    rules: Vec<ddl::FieldBuilder>,
    expected_version: Option<u64>,
) -> Result<QueryValue, String> {
    let rules: Vec<_> = rules.into_iter().map(|b| b.build()).collect();
    let mut b = Batch::new();
    b.id(1);
    let mut builder = ddl::set_table_schema(table).rules(rules);
    if let Some(v) = expected_version {
        builder = builder.expected_version(v);
    }
    b.set_table_schema(alias, builder);
    let resp = db
        .execute(db_name, &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.results[alias].records[0].as_value().as_ref().clone())
}

/// Insert a record in a **transactional** batch (so resolver is wired and
/// FK checks fire).
///
/// Returns `Err` if the batch itself errors OR if the transaction was aborted
/// (e.g. due to a validator FK violation — the transactional path wraps
/// validation errors as tx aborts with Ok response + aborted tx info).
async fn try_insert_tx(
    db: &ShamirDb,
    db_name: &str,
    table: &str,
    record: QueryValue,
) -> Result<(), String> {
    let mut b = Batch::new();
    b.id(1);
    b.transactional();
    b.insert("ins", insert(table).row(record));
    let resp = db
        .execute(db_name, &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())?;

    // In the transactional path, a validator failure causes a tx abort
    // (the batch returns Ok with empty results and tx status "aborted").
    // We must check the transaction info to detect this.
    if let Some(ref tx_info) = resp.transaction {
        if !tx_info.is_committed() {
            if let Some(ref reason) = tx_info.reason {
                return Err(reason.clone());
            }
            return Err("transaction aborted (no reason)".into());
        }
    }

    Ok(())
}

/// Helper: set up db + repo with parent + child tables.
async fn setup_with_parent_index(sys_path: std::path::PathBuf) -> ShamirDb {
    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    db.create_db("testdb").await;

    // Create repo with both tables.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo(
        "cr",
        ddl::create_repo("main").tables(["departments", "employees"]),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Create an index on departments.dept_id.
    let mut b = Batch::new();
    b.id(2);
    b.create_index(
        "idx",
        ddl::create_index("dept_id_idx", "departments").field("dept_id"),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Insert a parent row.
    let mut b = Batch::new();
    b.id(3);
    b.insert(
        "ins",
        insert("departments").row(mpack!({
            "dept_id": 100,
            "name": "Engineering"
        })),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    db
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: FK accept (existing ref) and reject (non-existing ref)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fk_accept_existing_reject_missing() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_with_parent_index(sys_path).await;

    // Set FK schema on employees: dept_id references departments.dept_id.
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "employees",
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["dept_id"])
                .int()
                .required()
                .foreign_key("departments", "dept_id"),
        ],
        None,
    )
    .await;
    assert!(
        result.is_ok(),
        "set_table_schema with FK should succeed: {:?}",
        result.err()
    );

    // Valid: dept_id=100 exists in parent.
    let ok = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Alice",
            "dept_id": 100
        }),
    )
    .await;
    assert!(
        ok.is_ok(),
        "FK ref to existing dept_id=100 should pass: {:?}",
        ok.err()
    );

    // Invalid: dept_id=999 does NOT exist in parent.
    let bad = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Bob",
            "dept_id": 999
        }),
    )
    .await;
    assert!(
        bad.is_err(),
        "FK ref to non-existing dept_id=999 should be rejected"
    );
    let err = bad.unwrap_err();
    assert!(
        err.contains("fk_violation"),
        "error should contain fk_violation, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2: FK without index → fk_requires_index
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fk_without_index_rejected_at_ddl() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    db.create_db("testdb").await;

    // Create repo with tables but NO index on departments.dept_id.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo(
        "cr",
        ddl::create_repo("main").tables(["departments", "employees"]),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Try to set FK schema without an index on the parent field.
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "employees",
        vec![ddl::field(["dept_id"])
            .int()
            .required()
            .foreign_key("departments", "dept_id")],
        None,
    )
    .await;
    assert!(
        result.is_err(),
        "FK without index should be rejected at DDL time"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("fk_requires_index"),
        "error should contain fk_requires_index, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: FK survives durable reopen
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fk_survives_durable_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // Session 1: set up parent + child with FK, insert valid row.
    {
        let db = setup_with_parent_index(sys_path.clone()).await;

        let result = exec_set_schema(
            &db,
            "testdb",
            "ss",
            "employees",
            vec![
                ddl::field(["name"]).string().required(),
                ddl::field(["dept_id"])
                    .int()
                    .required()
                    .foreign_key("departments", "dept_id"),
            ],
            None,
        )
        .await;
        assert!(result.is_ok());

        // Valid insert.
        let ok = try_insert_tx(
            &db,
            "testdb",
            "employees",
            mpack!({
                "name": "Alice",
                "dept_id": 100
            }),
        )
        .await;
        assert!(ok.is_ok(), "valid FK insert in session 1: {:?}", ok.err());
    }
    // ShamirDb dropped.

    // Session 2: reopen, FK schema should survive.
    let db = reopen_durable(sys_path).await;

    // Invalid: dept_id=999 does not exist → must still be rejected.
    let bad = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Carol",
            "dept_id": 999
        }),
    )
    .await;
    assert!(
        bad.is_err(),
        "FK should survive reopen: non-existing dept_id must still be rejected"
    );
    let err = bad.unwrap_err();
    assert!(
        err.contains("fk_violation"),
        "error should contain fk_violation after reopen, got: {err}"
    );

    // Valid: dept_id=100 still works.
    let ok = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Dave",
            "dept_id": 100
        }),
    )
    .await;
    assert!(
        ok.is_ok(),
        "valid FK insert after reopen should pass: {:?}",
        ok.err()
    );
}
