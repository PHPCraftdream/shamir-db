//! DDL-time rejection of self-referential `ON DELETE CASCADE`.
//!
//! The DELETE-path cascade planner (`fk_actions.rs::plan_cascade_recursive`)
//! uses a table-NAME-based per-path cycle guard (`CascadePathGuard`). A
//! self-referential CASCADE re-enters the same table name on the very first
//! recursion level — which the guard cannot distinguish from a genuine
//! cross-table cycle — and is therefore silently skipped at runtime as
//! defense-in-depth. The `validate_no_self_referential_cascade` DDL-time guard
//! converts that silent skip into an explicit, honest error so an operator
//! never declares an action the engine cannot honor.
//!
//! These tests verify the rejection fires at DDL time (before any delete is
//! attempted), via both `set_table_schema` and `add_schema_rule`, while
//! self-referential `SET NULL` and non-self-referential `CASCADE` continue to
//! be accepted.

use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::FkAction;

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

/// Execute a `set_table_schema` DDL op and return the response result or error.
async fn exec_set_schema(
    db: &ShamirDb,
    alias: &str,
    table: &str,
    rules: Vec<ddl::FieldBuilder>,
) -> Result<shamir_types::types::value::QueryValue, String> {
    let rules: Vec<_> = rules.into_iter().map(|b| b.build()).collect();
    let mut b = Batch::new();
    b.id(1);
    b.set_table_schema(alias, ddl::set_table_schema(table).rules(rules));
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.results[alias].records[0].as_value().as_ref().clone())
}

/// Execute an `add_schema_rule` DDL op and return the response result or error.
async fn exec_add_schema_rule(
    db: &ShamirDb,
    alias: &str,
    table: &str,
    rule: ddl::FieldBuilder,
) -> Result<shamir_types::types::value::QueryValue, String> {
    let rule = rule.build();
    let mut b = Batch::new();
    b.id(1);
    b.add_schema_rule(alias, ddl::add_schema_rule(table).rule(rule));
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.results[alias].records[0].as_value().as_ref().clone())
}

/// Set up a db + repo with a single `employees` table and an index on `id`
/// (the FK target — `validate_fk_indexes` requires it). Returns the db and the
/// tempdir (caller must hold it).
async fn setup() -> (ShamirDb, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    db.create_db("testdb").await;

    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["employees"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Index on employees.id — the FK target field.
    let mut b = Batch::new();
    b.id(2);
    b.create_index("idx", ddl::create_index("id_idx", "employees").field("id"));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    (db, dir)
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: Self-referential ON DELETE CASCADE → rejected via set_table_schema
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn self_ref_cascade_rejected_via_set_table_schema() {
    let (db, _dir) = setup().await;

    let result = exec_set_schema(
        &db,
        "ss",
        "employees",
        vec![ddl::field(["manager_id"])
            .int()
            .nullable()
            .foreign_key_on_delete("employees", "id", FkAction::Cascade)],
    )
    .await;

    assert!(
        result.is_err(),
        "self-referential CASCADE should be rejected at DDL time"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("self_referential_cascade_not_supported"),
        "error should contain self_referential_cascade_not_supported, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2: Self-referential ON DELETE CASCADE → rejected via add_schema_rule
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn self_ref_cascade_rejected_via_add_schema_rule() {
    let (db, _dir) = setup().await;

    let result = exec_add_schema_rule(
        &db,
        "ar",
        "employees",
        ddl::field(["manager_id"])
            .int()
            .nullable()
            .foreign_key_on_delete("employees", "id", FkAction::Cascade),
    )
    .await;

    assert!(
        result.is_err(),
        "self-referential CASCADE should be rejected at DDL time via add_schema_rule"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("self_referential_cascade_not_supported"),
        "error should contain self_referential_cascade_not_supported, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: Self-referential ON DELETE SET NULL → accepted (not rejected)
//
// SET NULL is single-level (never recurses) and is safe for self-referential
// FKs — it must NOT be rejected by the new guard.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn self_ref_set_null_accepted_at_ddl() {
    let (db, _dir) = setup().await;

    let result = exec_set_schema(
        &db,
        "ss",
        "employees",
        vec![ddl::field(["manager_id"])
            .int()
            .nullable()
            .foreign_key_on_delete("employees", "id", FkAction::SetNull)],
    )
    .await;

    assert!(
        result.is_ok(),
        "self-referential SET NULL should be accepted at DDL time: {:?}",
        result.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 4: Regression — non-self-referential CASCADE still accepted
//
// The guard must only reject CASCADE where ref_table == the table being
// defined. A normal cross-table CASCADE must pass.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn non_self_ref_cascade_still_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    db.create_db("testdb").await;

    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["parent", "child"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Index on parent.id (FK target).
    let mut b = Batch::new();
    b.id(2);
    b.create_index("idx", ddl::create_index("id_idx", "parent").field("id"));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let result = exec_set_schema(
        &db,
        "ss",
        "child",
        vec![ddl::field(["parent_id"])
            .int()
            .nullable()
            .foreign_key_on_delete("parent", "id", FkAction::Cascade)],
    )
    .await;

    assert!(
        result.is_ok(),
        "non-self-referential CASCADE should still be accepted: {:?}",
        result.err()
    );
}
