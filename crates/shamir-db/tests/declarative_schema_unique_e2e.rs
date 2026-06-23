//! Phase C3 — unique declarative constraint e2e tests.
//!
//! Exercises the REAL DDL execution path: create table with index, set unique
//! schema, insert rows with existing / non-existing duplicate values, and
//! verify unique enforcement survives durable reopen.
//!
//! Unique checks require `ctx.db() == Some`, which means the insert must go
//! through the **transactional** batch path (where the ValidatorDb is wired).
//! Autocommit (implicit tx) does NOT wire the ValidatorDb, so unique is
//! silently skipped there (same precedent as FK).

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

/// Insert a record in a **transactional** batch (so ValidatorDb is wired
/// and unique checks fire).
///
/// Returns `Err` if the batch itself errors OR if the transaction was aborted
/// (e.g. due to a validator unique violation — the transactional path wraps
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

    // In the transactional path, a validator failure causes a tx abort.
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

/// Insert TWO records via two separate insert statements in a single
/// transactional batch (multi-statement tx — staged-probe test).
///
/// Statement 1 stages row 1, then statement 2's validator probes staged
/// writes and detects the duplicate.
async fn try_insert_tx_two_stmts(
    db: &ShamirDb,
    db_name: &str,
    table: &str,
    r1: QueryValue,
    r2: QueryValue,
) -> Result<(), String> {
    let mut b = Batch::new();
    b.id(1);
    b.transactional();
    b.insert("ins1", insert(table).row(r1));
    b.insert("ins2", insert(table).row(r2));
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

/// Helper: set up db + repo with a table and an index on a column.
async fn setup_with_index(sys_path: std::path::PathBuf) -> ShamirDb {
    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    db.create_db("testdb").await;

    // Create repo with table.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["users"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Create an index on users.email.
    let mut b = Batch::new();
    b.id(2);
    b.create_index(
        "idx",
        ddl::create_index("email_idx", "users").field("email"),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    db
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: Unique accept (no duplicate) and reject (duplicate existing)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn unique_accept_new_reject_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_with_index(sys_path).await;

    // Set unique schema on users.email.
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["email"]).string().required().unique(),
        ],
        None,
    )
    .await;
    assert!(
        result.is_ok(),
        "set_table_schema with unique should succeed: {:?}",
        result.err()
    );

    // First insert: email="alice@test.com" — no duplicate.
    let ok = try_insert_tx(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Alice",
            "email": "alice@test.com"
        }),
    )
    .await;
    assert!(
        ok.is_ok(),
        "first insert of unique email should pass: {:?}",
        ok.err()
    );

    // Second insert with SAME email — should be rejected.
    let bad = try_insert_tx(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Bob",
            "email": "alice@test.com"
        }),
    )
    .await;
    assert!(bad.is_err(), "duplicate unique email should be rejected");
    let err = bad.unwrap_err();
    assert!(
        err.contains("unique_violation"),
        "error should contain unique_violation, got: {err}"
    );

    // Third insert with different email — should pass.
    let ok2 = try_insert_tx(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Carol",
            "email": "carol@test.com"
        }),
    )
    .await;
    assert!(
        ok2.is_ok(),
        "different unique email should pass: {:?}",
        ok2.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2: Unique without index → unique_requires_index
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn unique_without_index_rejected_at_ddl() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    db.create_db("testdb").await;

    // Create repo with table but NO index on users.email.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["users"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Try to set unique schema without an index on the field.
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![ddl::field(["email"]).string().required().unique()],
        None,
    )
    .await;
    assert!(
        result.is_err(),
        "unique without index should be rejected at DDL time"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("unique_requires_index"),
        "error should contain unique_requires_index, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: Batch-duplicate within a single tx (read-your-own-writes)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn unique_batch_duplicate_within_tx() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_with_index(sys_path).await;

    // Set unique schema on users.email.
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["email"]).string().required().unique(),
        ],
        None,
    )
    .await;
    assert!(result.is_ok());

    // Insert two records with the SAME email via two separate statements
    // in a single transactional batch. Statement 1 stages row 1, then
    // statement 2's validator probes staged writes (read-your-own-writes)
    // and detects the duplicate.
    let bad = try_insert_tx_two_stmts(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Alice",
            "email": "dup@test.com"
        }),
        mpack!({
            "name": "Bob",
            "email": "dup@test.com"
        }),
    )
    .await;
    assert!(
        bad.is_err(),
        "multi-statement duplicate unique email should be rejected within one tx"
    );
    let err = bad.unwrap_err();
    assert!(
        err.contains("unique_violation"),
        "error should contain unique_violation for staged-probe dup, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 4: Unique survives durable reopen
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn unique_survives_durable_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // Session 1: set up table with unique schema, insert valid row.
    {
        let db = setup_with_index(sys_path.clone()).await;

        let result = exec_set_schema(
            &db,
            "testdb",
            "ss",
            "users",
            vec![
                ddl::field(["name"]).string().required(),
                ddl::field(["email"]).string().required().unique(),
            ],
            None,
        )
        .await;
        assert!(result.is_ok());

        // Valid insert.
        let ok = try_insert_tx(
            &db,
            "testdb",
            "users",
            mpack!({
                "name": "Alice",
                "email": "alice@test.com"
            }),
        )
        .await;
        assert!(
            ok.is_ok(),
            "valid unique insert in session 1: {:?}",
            ok.err()
        );
    }
    // ShamirDb dropped.

    // Session 2: reopen, unique schema should survive.
    let db = reopen_durable(sys_path).await;

    // Duplicate: email="alice@test.com" exists → must still be rejected.
    let bad = try_insert_tx(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Bob",
            "email": "alice@test.com"
        }),
    )
    .await;
    assert!(
        bad.is_err(),
        "unique should survive reopen: duplicate email must still be rejected"
    );
    let err = bad.unwrap_err();
    assert!(
        err.contains("unique_violation"),
        "error should contain unique_violation after reopen, got: {err}"
    );

    // New unique email — should pass.
    let ok = try_insert_tx(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Carol",
            "email": "carol@test.com"
        }),
    )
    .await;
    assert!(
        ok.is_ok(),
        "valid unique insert after reopen should pass: {:?}",
        ok.err()
    );
}
