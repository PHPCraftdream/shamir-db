//! Phase C3 — unique declarative constraint e2e tests.
//!
//! Exercises the REAL DDL execution path: create table with index, set unique
//! schema, insert rows with existing / non-existing duplicate values, and
//! verify unique enforcement survives durable reopen.
//!
//! ## Two-layer unique contract (②.3b defense-in-depth)
//!
//! Unique is enforced by TWO complementary layers (see the normative doc block
//! in `schema_validator.rs` Phase C3 and `table_manager.rs::unique_write_lock`):
//!
//! 1. **Logical probe** (`schema_validator.rs`) — fail-fast, produces a clean
//!    field-scoped `unique_violation`. Runs on BOTH transactional and autocommit
//!    writes (the autocommit path routes through `run_implicit_batch_tx`, so
//!    `ctx.db()` is `Some`). NULL-bypass + UPDATE-skip-if-unchanged are here.
//! 2. **Physical index-guard** (`unique_write_lock` HIGH-A + commit-phase
//!    dedup) — the atomicity authority, closes the non-tx ↔ tx race.
//!
//! These tests exercise the contract end-to-end through the real engine.

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

/// Insert a record on the **autocommit** (non-transactional) batch path.
///
/// The autocommit path routes through `run_implicit_batch_tx`, which threads
/// a tx → `ctx.db()` is `Some` → the unique probe fires (unlike FK, which
/// additionally needs a cross-table resolver). A unique violation surfaces as
/// a `BatchError::QueryError` carrying `"unique_violation"` in its message
/// (from `DbError::ValidatorRejected`), so we propagate via `?` and the
/// caller substring-matches the error string.
async fn try_insert_autocommit(
    db: &ShamirDb,
    db_name: &str,
    table: &str,
    record: QueryValue,
) -> Result<(), String> {
    let mut b = Batch::new();
    b.id(1);
    // NOTE: deliberately NOT calling b.transactional() — autocommit path.
    b.insert("ins", insert(table).row(record));
    db.execute(db_name, &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())?;
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

// ═══════════════════════════════════════════════════════════════════════
// ②.3b Coherence tests — defense-in-depth unique contract
// ═══════════════════════════════════════════════════════════════════════
//
// These close the gaps NOT already covered by the four existing tests above
// (unique_accept_new_reject_duplicate, unique_without_index_rejected_at_ddl,
// unique_batch_duplicate_within_tx, unique_survives_durable_reopen) or by
// declarative_schema_relational_e2e::update_unique_violation_rejects.
//
// Covered here:
//   • NULL-bypass — unique does not constrain NULL (SQL semantics).
//   • UPDATE-skip-if-unchanged — updating a non-unique field on a row whose
//     unique value is untouched does NOT fire a false unique_violation.
//   • autocommit-enforcement — the probe fires on the non-tx path too.
//   • bare-index-without-rule — a `create_index{unique}` WITHOUT a schema
//     `unique` rule still rejects duplicates (index-guard enforces physically).
//
// NOTE on update-skip: `update_non_constrained_field_ok` in
// declarative_schema_relational_e2e.rs covers the SAME probe-skip logic but
// on the `employees`/`badge` table. The test below adds the `users`/`email`
// variant for local coherence. Not a semantic duplicate — different table.

// ───────────────────────────────────────────────────────────────────────
// Test 5: NULL-bypass — unique does not constrain NULL
// ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn unique_null_bypass_allows_multiple_nulls() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_with_index(sys_path).await;

    // Schema: email is unique but nullable (NOT required) → NULL is permitted.
    let result = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["email"]).string().nullable().unique(),
        ],
        None,
    )
    .await;
    assert!(
        result.is_ok(),
        "schema setup should succeed: {:?}",
        result.err()
    );

    // First insert with email = NULL. The `mpack!` macro does not accept
    // bare enum-variant expressions, so build the map manually.
    let null_email_alice = {
        let mut m = shamir_types::types::common::new_map();
        m.insert("name".to_string(), QueryValue::Str("Alice".into()));
        m.insert("email".to_string(), QueryValue::Null);
        QueryValue::Map(m)
    };
    let ok1 = try_insert_tx(&db, "testdb", "users", null_email_alice).await;
    assert!(
        ok1.is_ok(),
        "first NULL-email insert should pass (unique bypasses NULL): {:?}",
        ok1.err()
    );

    // Second insert with email = NULL — must ALSO pass (NULL ≠ NULL for unique).
    let null_email_bob = {
        let mut m = shamir_types::types::common::new_map();
        m.insert("name".to_string(), QueryValue::Str("Bob".into()));
        m.insert("email".to_string(), QueryValue::Null);
        QueryValue::Map(m)
    };
    let ok2 = try_insert_tx(&db, "testdb", "users", null_email_bob).await;
    assert!(
        ok2.is_ok(),
        "second NULL-email insert should pass (unique bypasses NULL): {:?}",
        ok2.err()
    );
}

// ───────────────────────────────────────────────────────────────────────
// Test 6: UPDATE-skip-if-unchanged — no false positive when unique field
//         is not modified by the update.
// ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn unique_update_skip_if_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_with_index(sys_path).await;

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
        "schema setup should succeed: {:?}",
        result.err()
    );

    // Insert two distinct unique rows.
    let ok = try_insert_tx(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Alice",
            "email": "alice@skip.com"
        }),
    )
    .await;
    assert!(ok.is_ok(), "first insert: {:?}", ok.err());

    let ok2 = try_insert_tx(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Bob",
            "email": "bob@skip.com"
        }),
    )
    .await;
    assert!(ok2.is_ok(), "second insert: {:?}", ok2.err());

    // UPDATE Alice's name (NOT her email). The probe must skip — the new
    // email value equals the old committed email value, so no false
    // unique_violation should fire even though "alice@skip.com" exists.
    let mut b = Batch::new();
    b.id(1);
    b.transactional();
    b.update(
        "upd",
        update("users")
            .where_(eq("email", "alice@skip.com"))
            .set(mpack!({
                "name": "Alicia"
            })),
    );
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string());

    // The update should succeed — no tx abort.
    match resp {
        Ok(resp) => {
            if let Some(ref tx_info) = resp.transaction {
                assert!(
                    tx_info.is_committed(),
                    "update of non-unique field should commit, got abort: {:?}",
                    tx_info.reason
                );
            }
        }
        Err(e) => panic!("update of non-unique field should not error: {e}"),
    }
}

// ───────────────────────────────────────────────────────────────────────
// Test 7: autocommit-enforcement — probe fires on non-tx path
// ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn unique_autocommit_enforcement() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = setup_with_index(sys_path).await;

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
        "schema setup should succeed: {:?}",
        result.err()
    );

    // First insert on the AUTOCOMMIT path (no explicit tx).
    let ok = try_insert_autocommit(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Alice",
            "email": "alice@auto.com"
        }),
    )
    .await;
    assert!(
        ok.is_ok(),
        "first autocommit insert should pass: {:?}",
        ok.err()
    );

    // Second autocommit insert with the SAME email — the probe fires on
    // the implicit-tx path and rejects it.
    let bad = try_insert_autocommit(
        &db,
        "testdb",
        "users",
        mpack!({
            "name": "Bob",
            "email": "alice@auto.com"
        }),
    )
    .await;
    assert!(
        bad.is_err(),
        "duplicate autocommit insert should be rejected (probe fires on implicit tx)"
    );
    let err = bad.unwrap_err();
    assert!(
        err.contains("unique_violation"),
        "autocommit duplicate error should contain unique_violation, got: {err}"
    );
}

// ───────────────────────────────────────────────────────────────────────
// Test 8: bare-index-without-rule — create_index{unique} WITHOUT a schema
//         unique rule still enforces uniqueness (physical index-guard).
//
// This confirms the "reverse direction" of the DDL-invariant: the invariant
// requires unique-RULE ⟹ index, but a unique-INDEX without a rule is
// legitimate and the index-guard layer rejects duplicates physically.
// ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn unique_bare_index_without_rule_rejects_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    db.create_db("testdb").await;

    // Create repo with table.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["items"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Create a UNIQUE index on items.sku — but do NOT set a schema unique
    // rule (no set_table_schema call at all).
    let mut b = Batch::new();
    b.id(2);
    b.create_index(
        "idx",
        ddl::create_index("sku_idx", "items").field("sku").unique(),
    );
    let resp = db.execute("testdb", &b.to_request_via_msgpack()).await;
    assert!(
        resp.is_ok(),
        "create unique index should succeed: {:?}",
        resp.err()
    );

    // First insert — should pass.
    let ok = try_insert_autocommit(
        &db,
        "testdb",
        "items",
        mpack!({
            "sku": "WIDGET-001"
        }),
    )
    .await;
    assert!(
        ok.is_ok(),
        "first insert under bare unique index should pass: {:?}",
        ok.err()
    );

    // Second insert with the SAME sku — must be rejected by the index-guard
    // (there is no schema unique rule, so the probe does not fire; the
    // physical unique-index posting layer enforces it). The error surfaces
    // as a storage-level "Unique index ... violated" rather than the
    // probe's field-scoped "unique_violation" code — this distinction is
    // exactly the two-layer contract: the index-guard's own rejection path.
    let bad = try_insert_autocommit(
        &db,
        "testdb",
        "items",
        mpack!({
            "sku": "WIDGET-001"
        }),
    )
    .await;
    assert!(
        bad.is_err(),
        "duplicate under bare unique index should be rejected by index-guard"
    );
    let err = bad.unwrap_err();
    // The index-guard rejects at the storage layer with a "Unique index ...
    // violated" message (not the probe's "unique_violation" code, since no
    // schema rule wired the probe). Accept either spelling — the contract
    // is "duplicate rejected", regardless of which layer fires.
    assert!(
        err.contains("unique_violation") || err.contains("Unique index"),
        "bare-index duplicate should be rejected (probe or index-guard), got: {err}"
    );
}
