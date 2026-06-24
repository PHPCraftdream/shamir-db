//! Phase D — `ON DELETE` referential actions through the REAL catalogue-compile
//! DDL path. Regression coverage for bug #236.
//!
//! Root cause #236 (found via this suite): the catalogue **writer**
//! (`admin_schema::insert_constraint_fields`) serialised only `ref_table` /
//! `ref_field` and dropped `on_delete`, so every FK read back as `NoAction` —
//! silently disabling ALL reverse-FK discovery (RESTRICT delete-guard, CASCADE,
//! SET NULL). A second bug made the DropTable guard read the in-memory
//! validator-binding cache, which is incoherent between the admin `DbInstance`
//! and the engine execute-path instance; it now reads the persisted catalogue.
//!
//! The engine unit tests use an in-memory `SchemaValidator` and never exercise
//! the catalogue round-trip, so they could not catch either bug. This suite
//! drives `db.execute(...)` — the exact path the server uses — and so is the
//! faithful, fast (no server rebuild) regression guard.
//!
//! Parent = `departments` (pk-ish `dept_id`, indexed). Child = `employees`
//! with `dept_id` FK → `departments.dept_id`.

use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::{delete, insert};
use shamir_query_types::admin::FkAction;
use shamir_types::mpack;

/// Set up db + repo with parent (indexed) + child (indexed on FK field),
/// one parent row `dept_id=100`.
async fn setup(sys_path: std::path::PathBuf) -> ShamirDb {
    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    db.create_db("testdb").await;

    let mut b = Batch::new();
    b.id(1);
    b.create_repo(
        "cr",
        ddl::create_repo("main").tables(["departments", "employees"]),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Index on departments.dept_id (FK target) + employees.dept_id (FK field).
    let mut b = Batch::new();
    b.id(2);
    b.create_index(
        "idx_p",
        ddl::create_index("dept_id_idx", "departments").field("dept_id"),
    );
    b.create_index(
        "idx_c",
        ddl::create_index("emp_dept_idx", "employees").field("dept_id"),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Parent row.
    let mut b = Batch::new();
    b.id(3);
    b.insert(
        "ins",
        insert("departments").row(mpack!({ "dept_id": 100, "name": "Engineering" })),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    db
}

/// Set the child FK schema with a chosen on-delete action. `nullable` controls
/// whether the FK field is declared nullable (required for SET NULL).
async fn set_child_fk_schema(db: &ShamirDb, action: FkAction, nullable: bool) {
    let mut field = ddl::field(["dept_id"]).int();
    field = if nullable {
        field.nullable()
    } else {
        field.required()
    };
    let field = field.foreign_key_on_delete("departments", "dept_id", action);

    let mut b = Batch::new();
    b.id(10);
    b.set_table_schema(
        "ss",
        ddl::set_table_schema("employees").rules(vec![field.build()]),
    );
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("set_table_schema execute");
    // Surface a DDL-level rejection (e.g. fk_requires_index) loudly.
    assert!(
        resp.results.contains_key("ss"),
        "set_table_schema produced no result for alias 'ss': {resp:?}"
    );
}

/// Insert a child row referencing `dept_id` in a transactional batch (so the
/// forward-FK check sees the wired resolver).
async fn insert_child_tx(db: &ShamirDb, dept_id: i64, name: &str) {
    let mut b = Batch::new();
    b.id(20);
    b.transactional();
    b.insert(
        "ins",
        insert("employees").row(mpack!({
            "dept_id": @(shamir_types::types::value::QueryValue::Int(dept_id)),
            "name": @(shamir_types::types::value::QueryValue::Str(name.to_string()))
        })),
    );
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("child insert execute");
    if let Some(ref tx) = resp.transaction {
        assert!(
            tx.is_committed(),
            "child insert tx aborted: {:?}",
            tx.reason
        );
    }
}

/// Count `employees` rows matching `dept_id == value` (or all rows if `None`).
async fn count_employees(db: &ShamirDb, dept_id: Option<i64>) -> usize {
    let mut b = Batch::new();
    b.id(30);
    let q = match dept_id {
        Some(v) => Query::from("employees").where_eq("dept_id", v),
        None => Query::from("employees"),
    };
    b.query("q", q);
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("count query execute");
    resp.results["q"].records.len()
}

/// Delete the parent `dept_id=100`. `transactional` selects the explicit-tx
/// path (resolver wired) vs the autocommit/implicit path.
async fn delete_parent(
    db: &ShamirDb,
    transactional: bool,
) -> Result<shamir_query_types::batch::BatchResponse, String> {
    let mut b = Batch::new();
    b.id(40);
    if transactional {
        b.transactional();
    }
    b.delete(
        "d",
        delete("departments").where_(shamir_query_builder::filter::eq("dept_id", 100)),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())
}

// ═══════════════════════════════════════════════════════════════════════
// CASCADE — transactional delete (mirrors the server's tx-wrapped path)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cascade_deletes_child_transactional() {
    let dir = tempfile::tempdir().unwrap();
    let db = setup(dir.path().join("meta.redb")).await;
    set_child_fk_schema(&db, FkAction::Cascade, false).await;
    insert_child_tx(&db, 100, "Alice").await;

    assert_eq!(
        count_employees(&db, Some(100)).await,
        1,
        "child present pre-delete"
    );

    delete_parent(&db, true)
        .await
        .expect("parent delete should succeed");

    assert_eq!(
        count_employees(&db, Some(100)).await,
        0,
        "CASCADE: child should be deleted after parent delete (transactional)"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// CASCADE — non-transactional (autocommit/implicit) delete
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cascade_deletes_child_autocommit() {
    let dir = tempfile::tempdir().unwrap();
    let db = setup(dir.path().join("meta.redb")).await;
    set_child_fk_schema(&db, FkAction::Cascade, false).await;
    insert_child_tx(&db, 100, "Alice").await;

    delete_parent(&db, false)
        .await
        .expect("parent delete should succeed");

    assert_eq!(
        count_employees(&db, Some(100)).await,
        0,
        "CASCADE: child should be deleted after parent delete (autocommit)"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// SET NULL — child survives with FK nulled
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn set_null_nulls_child_fk_transactional() {
    let dir = tempfile::tempdir().unwrap();
    let db = setup(dir.path().join("meta.redb")).await;
    set_child_fk_schema(&db, FkAction::SetNull, true).await;
    insert_child_tx(&db, 100, "Alice").await;

    delete_parent(&db, true)
        .await
        .expect("parent delete should succeed");

    // Child survives.
    assert_eq!(
        count_employees(&db, None).await,
        1,
        "SET NULL: child row should survive parent delete"
    );
    // Its FK is now null (no longer matches dept_id=100).
    assert_eq!(
        count_employees(&db, Some(100)).await,
        0,
        "SET NULL: child FK should be nulled (not match old parent value)"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// RESTRICT delete-guard — control: this is the path the TS e2e proves green.
// If THIS fails in-process too, the harness setup differs from the server;
// if it passes while cascade/drop fail, the divergence is path-specific.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn restrict_delete_guard_control() {
    let dir = tempfile::tempdir().unwrap();
    let db = setup(dir.path().join("meta.redb")).await;
    set_child_fk_schema(&db, FkAction::Restrict, false).await;
    insert_child_tx(&db, 100, "Alice").await;

    let resp = delete_parent(&db, false).await;
    let refused = format!("{resp:?}").contains("fk_restrict");
    assert!(
        refused,
        "RESTRICT delete-guard should refuse parent delete (fk_restrict); got: {resp:?}"
    );
    assert_eq!(
        count_employees(&db, Some(100)).await,
        1,
        "RESTRICT: child must survive the refused delete"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// drop-guard — DropTable on a referenced parent is refused
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_guard_refuses_referenced_parent() {
    let dir = tempfile::tempdir().unwrap();
    let db = setup(dir.path().join("meta.redb")).await;
    set_child_fk_schema(&db, FkAction::Restrict, false).await;

    let mut b = Batch::new();
    b.id(50);
    b.drop_table("dt", ddl::drop_table("departments"));
    // The drop must be refused with drop_refused_fk. The refusal surfaces as a
    // top-level Err from `execute` (a coded BatchError).
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string());
    let refused = format!("{resp:?}").contains("drop_refused_fk");
    assert!(
        refused,
        "drop-guard: dropping referenced parent should be refused (drop_refused_fk); got: {resp:?}"
    );
}
