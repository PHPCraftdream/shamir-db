//! Honest round-trip e2e for declarative schema DDL.
//!
//! These tests exercise the **real DDL execution path** — `SetTableSchema`
//! / `AddSchemaRule` / `RemoveSchemaRule` / `GetTableSchema` batch ops
//! executed by the public `ShamirDb::execute` entry point — and prove the
//! schema is persisted, survives a durable reopen, and is re-compiled by
//! `boot_compile_schemas`.
//!
//! This is distinct from `declarative_schema_e2e.rs`, which tests the
//! *engine* layer via direct `register_and_bind_schema*` (bypassing the DDL
//! and catalogue persistence). Both suites are needed: the engine suite
//! covers validation semantics; this suite covers the server execution and
//! durability contract.

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
/// briefly while the previous session's store still holds the file lock
/// (the MemBuffer-wrapped store releases the lock a few ms after the owning
/// `ShamirDb` is dropped).
///
/// Matches the retry-on-"Locked" pattern from `validators_lifecycle.rs`.
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
) -> QueryValue {
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
        .expect("set_table_schema should succeed");
    resp.results[alias].records[0].as_value().as_ref().clone()
}

/// Field accessors on a response `QueryValue::Map`.
fn r_bool(v: &QueryValue, k: &str) -> Option<bool> {
    v.get(k).and_then(|x| x.as_bool())
}
fn r_int(v: &QueryValue, k: &str) -> Option<i64> {
    v.get(k).and_then(|x| x.as_i64())
}

/// Execute an insert into `table` (default repo "main") and return Ok/Err.
async fn try_insert(
    db: &ShamirDb,
    db_name: &str,
    table: &str,
    record: shamir_types::types::value::QueryValue,
) -> Result<(), String> {
    let mut b = Batch::new();
    b.id(1);
    b.insert("ins", insert(table).row(record));
    db.execute(db_name, &b.to_request_via_msgpack())
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: set_table_schema persists and validates live writes
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn ddl_set_table_schema_persists_and_validates() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
        .await
        .unwrap();
    db.create_db("testdb").await;

    // Create a durable repo + table via wire DDL.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo(
        "cr",
        ddl::create_repo("main").tables(["users"]),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Set a schema: email required string, age int 0..=150.
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![
            ddl::field(["email"]).string().max(255).required(),
            ddl::field(["age"]).int().min(0).max(150),
        ],
        None,
    )
    .await;

    // The response should include ok=true and schema_version=1.
    assert_eq!(r_bool(&result, "ok"), Some(true));
    assert_eq!(r_int(&result, "schema_version"), Some(1));

    // Valid write — accepted.
    let ok = try_insert(
        &db,
        "testdb",
        "users",
        mpack!({"email": "alice@example.com", "age": 30}),
    )
    .await;
    assert!(ok.is_ok(), "valid write should succeed: {:?}", ok.err());

    // Invalid: missing required email.
    let bad = try_insert(&db, "testdb", "users", mpack!({"age": 25})).await;
    assert!(bad.is_err(), "missing required should be rejected");
    assert!(
        bad.unwrap_err().contains("missing_required"),
        "error should mention missing_required"
    );

    // Invalid: age out of range.
    let bad2 = try_insert(
        &db,
        "testdb",
        "users",
        mpack!({"email": "bob@example.com", "age": 200}),
    )
    .await;
    assert!(bad2.is_err(), "age > 150 should be rejected");
    assert!(bad2.unwrap_err().contains("out_of_range"));
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2 (CRITICAL): schema survives durable reopen
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn ddl_schema_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // Session 1: set up durable db/repo/table, set a schema, insert a valid row.
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

        // Set schema: email required.
        let _ = exec_set_schema(
            &db,
            "testdb",
            "ss",
            "users",
            vec![ddl::field(["email"]).string().required()],
            None,
        )
        .await;

        // Insert a valid row.
        let ok = try_insert(
            &db,
            "testdb",
            "users",
            mpack!({"email": "alice@example.com"}),
        )
        .await;
        assert!(ok.is_ok());
    }
    // ShamirDb dropped here — the file lock is released asynchronously.

    // Session 2: reopen on the SAME path. boot_compile_schemas must
    // re-build the schema validator from the persisted catalogue record.
    let db = reopen_durable(sys_path).await;

    // Without re-setting the schema, an invalid write (missing email) must
    // STILL be rejected — proving the schema survived the reopen.
    let bad = try_insert(&db, "testdb", "users", mpack!({"name": "bob"})).await;
    assert!(
        bad.is_err(),
        "schema should survive reopen: missing email must still be rejected"
    );
    assert!(
        bad.unwrap_err().contains("missing_required"),
        "error should mention missing_required after reopen"
    );

    // A valid write should still succeed.
    let ok = try_insert(
        &db,
        "testdb",
        "users",
        mpack!({"email": "carol@example.com"}),
    )
    .await;
    assert!(ok.is_ok(), "valid write should succeed after reopen");
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: expected_version optimistic concurrency
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn ddl_expected_version_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

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

    // First set: version goes 0 → 1.
    let r1 = exec_set_schema(
        &db,
        "testdb",
        "ss1",
        "users",
        vec![ddl::field(["email"]).string().required()],
        None,
    )
    .await;
    assert_eq!(r_int(&r1, "schema_version"), Some(1));

    // Stale expected_version=0 → conflict.
    let mut b = Batch::new();
    b.id(2);
    b.set_table_schema(
        "ss_stale",
        ddl::set_table_schema("users")
            .rules(vec![ddl::field(["email"]).string().required().build()])
            .expected_version(0),
    );
    let resp = db.execute("testdb", &b.to_request_via_msgpack()).await;
    assert!(resp.is_err(), "stale expected_version should error");
    let err = resp.unwrap_err().to_string();
    assert!(
        err.contains("version_conflict"),
        "error should contain version_conflict, got: {err}"
    );

    // Correct expected_version=1 → succeeds, version → 2.
    let r2 = exec_set_schema(
        &db,
        "testdb",
        "ss2",
        "users",
        vec![
            ddl::field(["email"]).string().required(),
            ddl::field(["age"]).int().min(0),
        ],
        Some(1),
    )
    .await;
    assert_eq!(r_int(&r2, "schema_version"), Some(2));
}

// ═══════════════════════════════════════════════════════════════════════
// Test 4: add_schema_rule / remove_schema_rule / get_table_schema
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn ddl_add_remove_get_schema() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

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

    // Initial schema: email required.
    let _ = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![ddl::field(["email"]).string().required()],
        None,
    )
    .await;

    // Add a rule: age int min 0.
    let mut b = Batch::new();
    b.id(2);
    b.add_schema_rule(
        "ar",
        ddl::add_schema_rule("users")
            .rule(ddl::field(["age"]).int().min(0).max(150)),
    );
    let resp = db.execute("testdb", &b.to_request_via_msgpack()).await.unwrap();
    let result = resp.results["ar"].records[0].as_value().as_ref().clone();
    assert_eq!(r_bool(&result, "ok"), Some(true));
    assert_eq!(r_int(&result, "schema_version"), Some(2));

    // Now age is required-ish: insert with age=-5 should fail.
    let neg_record = {
        let mut m = shamir_types::types::common::new_map();
        m.insert("email".to_string(), shamir_types::types::value::QueryValue::Str("x@x.com".into()));
        m.insert("age".to_string(), shamir_types::types::value::QueryValue::Int(-5));
        shamir_types::types::value::QueryValue::Map(m)
    };
    let bad = try_insert(&db, "testdb", "users", neg_record).await;
    assert!(bad.is_err());
    assert!(bad.unwrap_err().contains("out_of_range"));

    // Remove the age rule.
    let mut b = Batch::new();
    b.id(3);
    b.remove_schema_rule("rr", ddl::remove_schema_rule("users", ["age"]));
    let resp = db.execute("testdb", &b.to_request_via_msgpack()).await.unwrap();
    let result = resp.results["rr"].records[0].as_value().as_ref().clone();
    assert_eq!(r_bool(&result, "ok"), Some(true));
    assert_eq!(r_bool(&result, "removed"), Some(true));

    // Now age=-5 should pass (rule removed).
    let neg_record = {
        let mut m = shamir_types::types::common::new_map();
        m.insert("email".to_string(), shamir_types::types::value::QueryValue::Str("y@y.com".into()));
        m.insert("age".to_string(), shamir_types::types::value::QueryValue::Int(-5));
        shamir_types::types::value::QueryValue::Map(m)
    };
    let ok = try_insert(&db, "testdb", "users", neg_record).await;
    assert!(ok.is_ok(), "age rule removed, negative should pass");

    // Get table schema — should show only the email rule now.
    let mut b = Batch::new();
    b.id(4);
    b.get_table_schema("gs", ddl::get_table_schema("users"));
    let resp = db.execute("testdb", &b.to_request_via_msgpack()).await.unwrap();
    let result = resp.results["gs"].records[0].as_value().as_ref().clone();
    let schema = result.get("schema").and_then(|v| v.as_array()).unwrap();
    // Should have exactly 1 rule (email), age was removed.
    let path_has = |r: &QueryValue, name: &str| {
        r.get("path")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().any(|p| p.as_str() == Some(name)))
            .unwrap_or(false)
    };
    let has_email = schema.iter().any(|r| path_has(r, "email"));
    let has_age = schema.iter().any(|r| path_has(r, "age"));
    assert!(has_email, "email rule should be present");
    assert!(!has_age, "age rule should have been removed");
}

// ═══════════════════════════════════════════════════════════════════════
// Test 5: Phase B scalar/format/compare through DDL + reopen
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn ddl_phase_b_through_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // Session 1: set schema with Phase B fields.
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

        // Schema: email format, start <= end (cross-field).
        let _ = exec_set_schema(
            &db,
            "testdb",
            "ss",
            "users",
            vec![
                ddl::field(["email"])
                    .string()
                    .format("email")
                    .required(),
                ddl::field(["end"])
                    .int()
                    .compare(["start".to_string()], ">="),
            ],
            None,
        )
        .await;

        // Valid: email format ok, end >= start.
        let ok = try_insert(
            &db,
            "testdb",
            "users",
            mpack!({"email": "alice@example.com", "start": 10, "end": 20}),
        )
        .await;
        assert!(ok.is_ok(), "valid phase B write: {:?}", ok.err());

        // Invalid email format.
        let bad = try_insert(
            &db,
            "testdb",
            "users",
            mpack!({"email": "garbage", "start": 10, "end": 20}),
        )
        .await;
        assert!(bad.is_err(), "bad format should be rejected");
        assert!(bad.unwrap_err().contains("bad_format"));
    }

    // Session 2: reopen.
    let db = reopen_durable(sys_path).await;

    // Phase B rules should survive reopen.
    let bad = try_insert(
        &db,
        "testdb",
        "users",
        mpack!({"email": "still-garbage", "start": 10, "end": 20}),
    )
    .await;
    assert!(
        bad.is_err(),
        "format check should survive reopen"
    );
    assert!(bad.unwrap_err().contains("bad_format"));

    // Valid write still works.
    let ok = try_insert(
        &db,
        "testdb",
        "users",
        mpack!({"email": "bob@example.com", "start": 5, "end": 15}),
    )
    .await;
    assert!(ok.is_ok(), "valid write after reopen should pass");
}
