//! DDL-time rejection of nested-path (multi-segment) default/auto_now/
//! auto_now_add rules.
//!
//! The write path (`apply_defaults` / `apply_transforms` in `write_helpers.rs`)
//! silently skips multi-segment paths — an MVP single-segment gate. Before
//! this fix, a multi-segment `default`/`auto_now`/`auto_now_add` rule was
//! silently accepted at DDL time and then silently dropped on every
//! insert/update forever — an asymmetry with `validate_unique_indexes`, which
//! already rejects a multi-segment `unique` at DDL time.
//!
//! These tests verify that the new `validate_nested_path_transforms` DDL-time
//! guard closes that asymmetry: a multi-segment transform rule is rejected
//! with `nested_path_transform_not_supported`, while single-segment rules
//! (the working case) continue to be accepted and function exactly as before.

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

/// Execute an `add_schema_rule` DDL op and return the response result map.
async fn exec_add_schema_rule(
    db: &ShamirDb,
    db_name: &str,
    alias: &str,
    table: &str,
    rule: ddl::FieldBuilder,
) -> Result<QueryValue, String> {
    let rule = rule.build();
    let mut b = Batch::new();
    b.id(1);
    b.add_schema_rule(alias, ddl::add_schema_rule(table).rule(rule));
    let resp = db
        .execute(db_name, &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.results[alias].records[0].as_value().as_ref().clone())
}

/// Set up a db + repo with a table (no indexes needed for these tests).
/// Returns the db AND the tempdir (caller must hold the tempdir for the
/// test's lifetime — dropping it deletes the backing store).
async fn setup() -> (ShamirDb, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
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

    (db, dir)
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: Multi-segment `default` → rejected at DDL time via
//         set_table_schema
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn nested_path_default_rejected_at_ddl() {
    let (db, _dir) = setup().await;

    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![ddl::field(["address", "zip"])
            .string()
            .default(mpack!("00000"))],
        None,
    )
    .await;
    assert!(
        result.is_err(),
        "multi-segment default should be rejected at DDL time"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("nested_path_transform_not_supported"),
        "error should contain nested_path_transform_not_supported, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2: Multi-segment `auto_now` → rejected at DDL time
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn nested_path_auto_now_rejected_at_ddl() {
    let (db, _dir) = setup().await;

    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![ddl::field(["meta", "updated_at"]).int().auto_now()],
        None,
    )
    .await;
    assert!(
        result.is_err(),
        "multi-segment auto_now should be rejected at DDL time"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("nested_path_transform_not_supported"),
        "error should contain nested_path_transform_not_supported, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: Multi-segment `auto_now_add` → rejected at DDL time
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn nested_path_auto_now_add_rejected_at_ddl() {
    let (db, _dir) = setup().await;

    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![ddl::field(["meta", "created_at"]).int().auto_now_add()],
        None,
    )
    .await;
    assert!(
        result.is_err(),
        "multi-segment auto_now_add should be rejected at DDL time"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("nested_path_transform_not_supported"),
        "error should contain nested_path_transform_not_supported, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 4: Multi-segment `default` via add_schema_rule → rejected at DDL time
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn nested_path_default_rejected_via_add_schema_rule() {
    let (db, _dir) = setup().await;

    // First, set a valid single-segment schema so the table has a schema.
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![ddl::field(["name"]).string()],
        None,
    )
    .await;
    assert!(
        result.is_ok(),
        "initial valid schema should succeed: {:?}",
        result.err()
    );

    // Now try to add a multi-segment default rule.
    let result = exec_add_schema_rule(
        &db,
        "testdb",
        "ar",
        "users",
        ddl::field(["address", "zip"])
            .string()
            .default(mpack!("00000")),
    )
    .await;
    assert!(
        result.is_err(),
        "multi-segment default via add_schema_rule should be rejected at DDL time"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("nested_path_transform_not_supported"),
        "error should contain nested_path_transform_not_supported, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 5: Regression — single-segment default/auto_now/auto_now_add still
//         accepted and functional
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn single_segment_transforms_still_accepted_and_functional() {
    let (db, _dir) = setup().await;

    // Set a schema with single-segment default, auto_now, and auto_now_add.
    // All three must be accepted — the new nested-path guard must NOT
    // over-reject single-segment rules.
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![
            ddl::field(["name"]).string(),
            ddl::field(["status"]).string().default(mpack!("active")),
            ddl::field(["created_at"]).int().auto_now_add(),
            ddl::field(["updated_at"]).int().auto_now(),
        ],
        None,
    )
    .await;
    assert!(
        result.is_ok(),
        "single-segment transforms should be accepted: {:?}",
        result.err()
    );

    // Insert a record without status/created_at/updated_at — the
    // default/transform rules should stamp them. The transactional batch
    // should commit (not abort), proving the rules are live and functional.
    let mut b = Batch::new();
    b.id(2);
    b.transactional();
    b.insert("ins", insert("users").row(mpack!({ "name": "Alice" })));
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    if let Some(ref tx_info) = resp.transaction {
        assert!(
            tx_info.is_committed(),
            "insert with valid single-segment transforms should commit, \
             got: {:?}",
            tx_info.reason
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Test 6: Regression — unique's own nested-path rejection still works
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn nested_path_unique_rejection_still_works() {
    let (db, _dir) = setup().await;

    // A multi-segment unique rule should still be rejected by
    // validate_unique_indexes (we only added a sibling check, not modified it).
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![ddl::field(["address", "zip"]).string().unique()],
        None,
    )
    .await;
    assert!(
        result.is_err(),
        "multi-segment unique should still be rejected at DDL time"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("unique_requires_index"),
        "error should contain unique_requires_index, got: {err}"
    );
}
