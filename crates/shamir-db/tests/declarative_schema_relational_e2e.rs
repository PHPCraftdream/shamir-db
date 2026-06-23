//! Phase C4 — combined relational constraint e2e tests.
//!
//! Exercises FK + unique constraints **together** on the same child table:
//! one field has a foreign_key to a parent table, another field has a unique
//! constraint.  Both constraints must be satisfied for a write to succeed.
//!
//! Also tests concurrent transactional writes to prove the lock-free
//! validator path does not deadlock under parallel tx contention.

use std::sync::Arc;

use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::filter::eq;
use shamir_query_builder::write::{insert, update};
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

/// Insert a record in a **transactional** batch (so ValidatorDb is wired
/// and both FK + unique checks fire).
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

/// Update a record in a **transactional** batch (so ValidatorDb is wired
/// and both FK + unique checks fire on the updated record).
async fn try_update_tx(
    db: &ShamirDb,
    db_name: &str,
    table: &str,
    filter: shamir_query_types::filter::Filter,
    set_doc: QueryValue,
) -> Result<(), String> {
    let mut b = Batch::new();
    b.id(1);
    b.transactional();
    b.update("upd", update(table).where_(filter).set(set_doc));
    let resp = db
        .execute(db_name, &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())?;

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

/// Set up db + repo with parent (departments) + child (employees) tables,
/// indexes on departments.dept_id AND employees.badge (for unique),
/// plus an index on employees.dept_id (for FK lookup).
async fn setup_combined(sys_path: std::path::PathBuf) -> ShamirDb {
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

    // Index on departments.dept_id (needed for FK lookup).
    let mut b = Batch::new();
    b.id(2);
    b.create_index(
        "idx_dept",
        ddl::create_index("dept_id_idx", "departments").field("dept_id"),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Index on employees.badge (needed for unique constraint).
    let mut b = Batch::new();
    b.id(3);
    b.create_index(
        "idx_badge",
        ddl::create_index("badge_idx", "employees").field("badge"),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Insert two parent departments.
    let mut b = Batch::new();
    b.id(4);
    b.insert(
        "d1",
        insert("departments").row(mpack!({
            "dept_id": 10,
            "name": "Engineering"
        })),
    );
    b.insert(
        "d2",
        insert("departments").row(mpack!({
            "dept_id": 20,
            "name": "Marketing"
        })),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    db
}

/// Apply the combined FK + unique schema on the employees table.
async fn apply_combined_schema(db: &ShamirDb) {
    let result = exec_set_schema(
        db,
        "testdb",
        "ss",
        "employees",
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["dept_id"])
                .int()
                .required()
                .foreign_key("departments", "dept_id"),
            ddl::field(["badge"]).string().required().unique(),
        ],
        None,
    )
    .await;
    assert!(
        result.is_ok(),
        "set_table_schema with FK + unique should succeed: {:?}",
        result.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: Combined FK + unique — valid row passes both constraints
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn combined_valid_passes_both_constraints() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_combined(sys_path).await;
    apply_combined_schema(&db).await;

    // Valid: dept_id=10 exists, badge="A001" is unique.
    let ok = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Alice",
            "dept_id": 10,
            "badge": "A001"
        }),
    )
    .await;
    assert!(
        ok.is_ok(),
        "valid FK + unique insert should pass: {:?}",
        ok.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2: Combined — FK violation when ref doesn't exist
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn combined_fk_violation_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_combined(sys_path).await;
    apply_combined_schema(&db).await;

    // Invalid: dept_id=999 does not exist (badge is unique — no dup).
    let bad = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Bob",
            "dept_id": 999,
            "badge": "B001"
        }),
    )
    .await;
    assert!(
        bad.is_err(),
        "FK violation should reject even when unique is satisfied"
    );
    let err = bad.unwrap_err();
    assert!(
        err.contains("fk_violation"),
        "error should contain fk_violation, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: Combined — unique violation rejects even when FK is valid
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn combined_unique_violation_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_combined(sys_path).await;
    apply_combined_schema(&db).await;

    // First row: valid.
    let ok = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Alice",
            "dept_id": 10,
            "badge": "U001"
        }),
    )
    .await;
    assert!(ok.is_ok(), "first insert should pass: {:?}", ok.err());

    // Second row: same badge "U001" → unique violation (FK is fine).
    let bad = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Bob",
            "dept_id": 20,
            "badge": "U001"
        }),
    )
    .await;
    assert!(
        bad.is_err(),
        "unique violation should reject even when FK is satisfied"
    );
    let err = bad.unwrap_err();
    assert!(
        err.contains("unique_violation"),
        "error should contain unique_violation, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 4: Combined constraints survive durable reopen
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn combined_constraints_survive_durable_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // Session 1: set up and insert a valid row.
    {
        let db = setup_combined(sys_path.clone()).await;
        apply_combined_schema(&db).await;

        let ok = try_insert_tx(
            &db,
            "testdb",
            "employees",
            mpack!({
                "name": "Alice",
                "dept_id": 10,
                "badge": "R001"
            }),
        )
        .await;
        assert!(
            ok.is_ok(),
            "valid combined insert in session 1: {:?}",
            ok.err()
        );
    }
    // ShamirDb dropped — file lock released.

    // Session 2: reopen, both constraints must survive.
    let db = reopen_durable(sys_path).await;

    // FK violation after reopen: dept_id=999.
    let bad_fk = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Bob",
            "dept_id": 999,
            "badge": "R002"
        }),
    )
    .await;
    assert!(
        bad_fk.is_err(),
        "FK should survive reopen: non-existing dept_id must be rejected"
    );
    let err = bad_fk.unwrap_err();
    assert!(
        err.contains("fk_violation"),
        "error should contain fk_violation after reopen, got: {err}"
    );

    // Unique violation after reopen: badge="R001" already exists.
    let bad_uniq = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Carol",
            "dept_id": 10,
            "badge": "R001"
        }),
    )
    .await;
    assert!(
        bad_uniq.is_err(),
        "unique should survive reopen: duplicate badge must be rejected"
    );
    let err = bad_uniq.unwrap_err();
    assert!(
        err.contains("unique_violation"),
        "error should contain unique_violation after reopen, got: {err}"
    );

    // Valid insert after reopen: both constraints pass.
    let ok = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Dave",
            "dept_id": 10,
            "badge": "R003"
        }),
    )
    .await;
    assert!(
        ok.is_ok(),
        "valid combined insert after reopen should pass: {:?}",
        ok.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 5: Concurrent tx writes — no deadlock under contention
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn concurrent_tx_writes_no_deadlock() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = Arc::new(setup_combined(sys_path).await);
    apply_combined_schema(&db).await;

    // Spawn N concurrent transactional inserts, each with a valid FK
    // and a unique badge. The validator must read parent table (FK) and
    // self-table (unique) without deadlocking even under parallel
    // contention on the same table.
    let n = 10;
    let mut handles = Vec::with_capacity(n);
    for i in 0..n {
        let db = Arc::clone(&db);
        let badge = format!("CONC-{i:03}");
        // Alternate between dept_id 10 and 20 to exercise cross-table
        // FK reads from different tx contexts simultaneously.
        let dept_id = if i % 2 == 0 { 10i64 } else { 20i64 };
        let name = format!("Worker-{i}");
        handles.push(tokio::spawn(async move {
            try_insert_tx(
                &db,
                "testdb",
                "employees",
                mpack!({
                    "name": @QueryValue::from(name),
                    "dept_id": @QueryValue::Int(dept_id),
                    "badge": @QueryValue::from(badge)
                }),
            )
            .await
        }));
    }

    // All must complete (nextest timeout catches deadlocks).
    let mut ok_count = 0usize;
    for h in handles {
        let result = h.await.expect("task should not panic");
        // Under high contention some tx may be aborted due to
        // write-write conflict — that is acceptable. What matters is
        // that every task terminates (no deadlock).
        if result.is_ok() {
            ok_count += 1;
        }
    }
    // At least some must succeed (all unique badges + valid FKs).
    assert!(
        ok_count > 0,
        "at least one concurrent tx insert should succeed"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 6: Update — FK violation when changing dept_id to non-existing
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn update_fk_violation_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_combined(sys_path).await;
    apply_combined_schema(&db).await;

    // Insert a valid row.
    let ok = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Alice",
            "dept_id": 10,
            "badge": "UPD-001"
        }),
    )
    .await;
    assert!(ok.is_ok(), "initial insert should pass: {:?}", ok.err());

    // Update: change dept_id to non-existing 999 → FK violation.
    let bad = try_update_tx(
        &db,
        "testdb",
        "employees",
        eq("badge", "UPD-001"),
        mpack!({
            "dept_id": 999
        }),
    )
    .await;
    assert!(
        bad.is_err(),
        "update changing dept_id to non-existing ref should be rejected"
    );
    let err = bad.unwrap_err();
    assert!(
        err.contains("fk_violation"),
        "error should contain fk_violation on update, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 7: Update — unique violation when changing badge to duplicate
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn update_unique_violation_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_combined(sys_path).await;
    apply_combined_schema(&db).await;

    // Insert two valid rows.
    let ok = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Alice",
            "dept_id": 10,
            "badge": "UPD-A"
        }),
    )
    .await;
    assert!(ok.is_ok(), "first insert should pass: {:?}", ok.err());

    let ok2 = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Bob",
            "dept_id": 20,
            "badge": "UPD-B"
        }),
    )
    .await;
    assert!(ok2.is_ok(), "second insert should pass: {:?}", ok2.err());

    // Update Bob's badge to "UPD-A" → unique violation.
    let bad = try_update_tx(
        &db,
        "testdb",
        "employees",
        eq("badge", "UPD-B"),
        mpack!({
            "badge": "UPD-A"
        }),
    )
    .await;
    assert!(
        bad.is_err(),
        "update changing badge to duplicate should be rejected"
    );
    let err = bad.unwrap_err();
    assert!(
        err.contains("unique_violation"),
        "error should contain unique_violation on update, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 8: Update — no constraint change → ok
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn update_non_constrained_field_ok() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_combined(sys_path).await;
    apply_combined_schema(&db).await;

    // Insert a valid row.
    let ok = try_insert_tx(
        &db,
        "testdb",
        "employees",
        mpack!({
            "name": "Alice",
            "dept_id": 10,
            "badge": "NC-001"
        }),
    )
    .await;
    assert!(ok.is_ok(), "initial insert should pass: {:?}", ok.err());

    // Update only the name — neither FK nor unique field touched.
    let ok2 = try_update_tx(
        &db,
        "testdb",
        "employees",
        eq("badge", "NC-001"),
        mpack!({
            "name": "Alice Smith"
        }),
    )
    .await;
    assert!(
        ok2.is_ok(),
        "update of non-constrained field should pass: {:?}",
        ok2.err()
    );
}
