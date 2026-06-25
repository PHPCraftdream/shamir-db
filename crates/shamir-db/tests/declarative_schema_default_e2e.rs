//! Phase ②.4c — DEFAULT stamp-enforcement e2e tests.
//!
//! Exercises the REAL DDL execution path: create table, set schema with a
//! `default` rule, insert rows with / without / explicit-null on the default
//! field, and verify the stamp fires correctly (absent → default, explicit
//! value preserved, explicit NULL preserved, required+default passes,
//! replay-idempotent via durable reopen, no-default behaves as before).
//!
//! ## Invariant (DDL-EVOLUTION-PLAN §②.4a — variant B)
//!
//! Literal-default replay-safe by construction: the stamp fires ONLY for an
//! ABSENT field. After the first write the field is present (carrying the
//! stamped default), so reload/replay never re-stamps. Explicit values
//! (including explicit `Null`) are NEVER overwritten. Only INSERT (not
//! UPDATE) — the upsert-update branch is untouched.

use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

// ═══════════════════════════════════════════════════════════════════════
// Helpers (mirrors declarative_schema_unique_e2e.rs)
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

/// Insert a record on the **autocommit** (non-transactional) batch path.
async fn try_insert_autocommit(
    db: &ShamirDb,
    db_name: &str,
    table: &str,
    record: QueryValue,
) -> Result<(), String> {
    let mut b = Batch::new();
    b.id(1);
    b.insert("ins", insert(table).row(record));
    db.execute(db_name, &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Read all rows from a table.
async fn read_all(db: &ShamirDb, db_name: &str, table: &str) -> Vec<QueryValue> {
    let mut b = Batch::new();
    b.id(1);
    b.query("all", Query::from(table));
    let resp = db
        .execute(db_name, &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())
        .unwrap();
    resp.results["all"]
        .records
        .iter()
        .map(|r| r.as_value().into_owned())
        .collect()
}

/// Helper: set up db + repo with a `users` table (no index needed for default).
async fn setup() -> ShamirDb {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    // Leak the tempdir so the fjall store survives for the test duration.
    // (mirrors the pattern in declarative_schema_unique_e2e.rs which holds
    //  the TempDir via the test's local binding.)
    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    db.create_db("testdb").await;
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["users"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    std::mem::forget(dir);
    db
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: default on insert — absent field → default value
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn default_stamps_absent_field_on_insert() {
    let db = setup().await;

    // Schema: status has a default "active"; name required.
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["status"]).string().default(mpack!("active")),
        ],
        None,
    )
    .await;
    assert!(
        result.is_ok(),
        "set_table_schema with default should succeed: {:?}",
        result.err()
    );

    // Insert WITHOUT status → stamp should fill "active".
    let ok = try_insert_autocommit(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Alice"
        }),
    )
    .await;
    assert!(ok.is_ok(), "insert without default field: {:?}", ok.err());

    let rows = read_all(&db, "testdb", "users").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name"), Some(&mpack!("Alice")));
    assert_eq!(
        rows[0].get("status"),
        Some(&mpack!("active")),
        "absent field should be stamped with default 'active'"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2: explicit value NOT overwritten
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn default_does_not_override_explicit_value() {
    let db = setup().await;

    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["status"]).string().default(mpack!("active")),
        ],
        None,
    )
    .await;
    assert!(result.is_ok());

    // Insert WITH explicit status="pending" → default must NOT override.
    let ok = try_insert_autocommit(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Bob",
            "status": "pending"
        }),
    )
    .await;
    assert!(ok.is_ok(), "insert with explicit value: {:?}", ok.err());

    let rows = read_all(&db, "testdb", "users").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("status"),
        Some(&mpack!("pending")),
        "explicit value must NOT be overridden by default"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: explicit NULL NOT overwritten (KEYSTONE replay-safe invariant)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn default_does_not_override_explicit_null() {
    let db = setup().await;

    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![
            ddl::field(["name"]).string().required(),
            // status is nullable with a default; explicit NULL must survive.
            ddl::field(["status"])
                .string()
                .nullable()
                .default(mpack!("active")),
        ],
        None,
    )
    .await;
    assert!(result.is_ok());

    // Insert with explicit status = NULL → default must NOT override.
    let ok = try_insert_autocommit(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Carol",
            "status": null
        }),
    )
    .await;
    assert!(
        ok.is_ok(),
        "insert with explicit null on nullable+default field: {:?}",
        ok.err()
    );

    let rows = read_all(&db, "testdb", "users").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("status"),
        Some(&QueryValue::Null),
        "explicit NULL must NOT be overridden by default (keystone invariant)"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 4: required + default → passes (default satisfies required)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn required_plus_default_passes_on_absent_field() {
    let db = setup().await;

    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![
            ddl::field(["name"]).string().required(),
            // role is BOTH required AND has a default — absent field is
            // stamped before validation, so required is satisfied.
            ddl::field(["role"])
                .string()
                .required()
                .default(mpack!("user")),
        ],
        None,
    )
    .await;
    assert!(result.is_ok());

    // Insert WITHOUT role → default stamps "user" BEFORE validation,
    // so required passes (no missing_required error).
    let ok = try_insert_autocommit(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Dave"
        }),
    )
    .await;
    assert!(
        ok.is_ok(),
        "required+default absent field should pass (default satisfies required): {:?}",
        ok.err()
    );

    let rows = read_all(&db, "testdb", "users").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("role"),
        Some(&mpack!("user")),
        "required+default absent field should be stamped with default"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 5: replay-idempotent — stamped value survives durable reopen
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn default_stamp_survives_durable_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // Session 1: set schema with default, insert absent-field row.
    {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        db.create_db("testdb").await;
        let mut b = Batch::new();
        b.id(1);
        b.create_repo("cr", ddl::create_repo("main").tables(["users"]));
        db.execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        let result = exec_set_schema(
            &db,
            "testdb",
            "ss",
            "users",
            vec![
                ddl::field(["name"]).string().required(),
                ddl::field(["status"]).string().default(mpack!("active")),
            ],
            None,
        )
        .await;
        assert!(result.is_ok());

        let ok = try_insert_autocommit(
            &db,
            "testdb",
            "users",
            mpack!({
                "name": "Eve"
            }),
        )
        .await;
        assert!(ok.is_ok());

        // Verify stamp fired in session 1.
        let rows = read_all(&db, "testdb", "users").await;
        assert_eq!(rows[0].get("status"), Some(&mpack!("active")));
    }
    // ShamirDb dropped — fjall store flushes to disk.

    // Session 2: reopen — schema + data survive; stamped value unchanged.
    let db = reopen_durable(sys_path).await;

    let rows = read_all(&db, "testdb", "users").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("status"),
        Some(&mpack!("active")),
        "stamped default must survive durable reopen (replay-safe)"
    );
    assert_eq!(rows[0].get("name"), Some(&mpack!("Eve")));
}

// ═══════════════════════════════════════════════════════════════════════
// Test 6: no default → behavior unchanged (required still rejects absence)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn no_default_behaves_as_before() {
    let db = setup().await;

    // Schema with NO default: status required.
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["status"]).string().required(),
        ],
        None,
    )
    .await;
    assert!(result.is_ok());

    // Insert WITHOUT status → required should reject (no default to rescue).
    let bad = try_insert_autocommit(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Frank"
        }),
    )
    .await;
    assert!(
        bad.is_err(),
        "required field without default must still reject absence"
    );
    let err = bad.unwrap_err();
    assert!(
        err.contains("missing_required"),
        "error should contain missing_required, got: {err}"
    );

    // Verify nothing was persisted.
    let rows = read_all(&db, "testdb", "users").await;
    assert_eq!(rows.len(), 0, "rejected insert must not persist");
}
