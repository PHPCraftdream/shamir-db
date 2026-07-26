//! Phase C2 — foreign-key declarative constraint e2e test: AUTOCOMMIT path.
//!
//! F-28 Step 1 (#828) — regression test for the D2 bug: the autocommit
//! (implicit single-op batch, no explicit `BeginTx`/`CommitTx`) insert path
//! was passing `resolver = None` into `execute_insert_tx`, which made
//! `ValidatorDb::exists_in` unconditionally return `Ok(false)` (no cross-table
//! resolver wired) for EVERY foreign-key check on this path — regardless of
//! whether the referenced parent row genuinely existed.
//! `schema_validator.rs` treats `Ok(false)` as "referenced row does not
//! exist" and raises `fk_violation`, so an autocommit insert into a table
//! with a schema-level `foreign_key` constraint was rejected 100% of the
//! time, even when the reference was perfectly valid. This is the missing
//! counterpart to `declarative_schema_fk_e2e.rs`'s transactional-only
//! coverage (see that file's module doc — it deliberately wraps inserts in
//! transactional batches "so resolver is wired").
//!
//! After the F-28 Step 1 fix (straight-line `begin_implicit_batch_tx` /
//! `commit_implicit_batch_tx` + `Some(self.resolver)` on the 4 implicit
//! arms), a valid FK reference must be ACCEPTED on the plain autocommit path
//! too, and an invalid reference must still be REJECTED.

use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::write::insert;
use shamir_types::mpack;

/// Insert a record in a plain **autocommit** (non-transactional) batch — the
/// default single-batch-op path, NOT wrapped in `BeginTx`/`CommitTx`.
async fn try_insert_autocommit(
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

#[tokio::test]
async fn fk_autocommit_accepts_existing_rejects_missing() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    db.create_db("testdb").await;

    // Create repo with parent + child tables.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo(
        "cr",
        ddl::create_repo("main").tables(["departments", "employees"]),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Index on departments.dept_id (required for FK).
    let mut b = Batch::new();
    b.id(2);
    b.create_index(
        "idx",
        ddl::create_index("dept_id_idx", "departments").field("dept_id"),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Insert a valid parent row via autocommit.
    let parent = try_insert_autocommit(
        &db,
        "testdb",
        "departments",
        mpack!({
            "dept_id": 100,
            "name": "Engineering"
        }),
    )
    .await;
    assert!(
        parent.is_ok(),
        "autocommit parent insert should succeed: {:?}",
        parent.err()
    );

    // Set FK schema on employees: dept_id references departments.dept_id.
    let rules = vec![
        ddl::field(["name"]).string().required().build(),
        ddl::field(["dept_id"])
            .int()
            .required()
            .foreign_key("departments", "dept_id")
            .build(),
    ];
    let mut b = Batch::new();
    b.id(3);
    b.set_table_schema("ss", ddl::set_table_schema("employees").rules(rules));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Valid: dept_id=100 exists in parent — inserted via plain AUTOCOMMIT
    // batch (no BeginTx/CommitTx). Before the F-28 Step 1 fix this was
    // wrongly rejected with fk_violation because the resolver was None on
    // this path; after the fix it must succeed.
    let ok = try_insert_autocommit(
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
        "autocommit FK ref to existing dept_id=100 should pass: {:?}",
        ok.err()
    );

    // Invalid: dept_id=999 does NOT exist in parent — must still be rejected
    // on the autocommit path.
    let bad = try_insert_autocommit(
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
        "autocommit FK ref to non-existing dept_id=999 should be rejected"
    );
    let err = bad.unwrap_err();
    assert!(
        err.contains("fk_violation"),
        "error should contain fk_violation, got: {err}"
    );
}
