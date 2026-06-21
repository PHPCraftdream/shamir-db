//! End-to-end lifecycle tests for the validator engine (S1 + S2).
//!
//! S1 covers: create → present (by name + id) → duplicate name → error,
//! rename → old gone / new resolves / same id, drop (unbound) → removed,
//! drop (bound) → error + not removed, and persistence (catalogue
//! records survive across init).
//!
//! S2 covers: bind → list → is_bound true → drop refused, idempotent
//! re-bind → updated, unbind → gone → is_bound false → drop succeeds,
//! validation errors (bad priority / empty ops / unknown name),
//! persistence of bindings across table reload.
//!
//! All tests use `create_validator_from_wasm` with a trivial echo `.wasm`
//! built from WAT so no Rust toolchain is needed.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::validator::WriteOp;
use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_storage::error::DbError;

/// Identity-echo WAT matching the slice-2 ABI.
///
/// Exports `memory` (2 pages), `shamir_alloc` (bump allocator), and
/// `shamir_call` which echoes `[ptr, len)` back as the packed result.
const ECHO_WAT: &str = r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl
        (i64.extend_i32_u (local.get $ptr))
        (i64.const 32)
      )
      (i64.extend_i32_u (local.get $len))
    )
  )
)
"#;

fn echo_wasm() -> Vec<u8> {
    wat::parse_str(ECHO_WAT).expect("WAT parse failed")
}

// ── create + duplicate ──────────────────────────────────────────────────

#[tokio::test]
async fn create_validator_present_by_name_and_id() {
    let db = ShamirDb::init_memory().await.unwrap();

    let id = db
        .create_validator_from_wasm("v1", &echo_wasm(), false)
        .await
        .unwrap();

    // Registry has it by name.
    assert_eq!(db.validator_id("v1"), Some(id));

    // Registry has it by id.
    assert!(db.validators().get_by_id(&id).is_some());
}

#[tokio::test]
async fn create_validator_duplicate_name_rejected() {
    let db = ShamirDb::init_memory().await.unwrap();

    db.create_validator_from_wasm("v1", &echo_wasm(), false)
        .await
        .unwrap();

    let err = db
        .create_validator_from_wasm("v1", &echo_wasm(), false)
        .await
        .unwrap_err();

    match err {
        DbError::Validation(msg) => {
            assert!(
                msg.contains("already exists"),
                "expected 'already exists', got: {msg}"
            );
        }
        other => panic!("expected Validation error, got: {other}"),
    }
}

// ── rename ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn rename_validator_rekeys_name_preserves_id() {
    let db = ShamirDb::init_memory().await.unwrap();

    let id = db
        .create_validator_from_wasm("old_name", &echo_wasm(), false)
        .await
        .unwrap();

    db.rename_validator("old_name", "new_name").await.unwrap();

    // Old name is gone.
    assert_eq!(db.validator_id("old_name"), None);

    // New name resolves to the same id.
    assert_eq!(db.validator_id("new_name"), Some(id));

    // Compiled artifact is still accessible.
    assert!(db.validators().get_by_id(&id).is_some());
}

// ── drop (unbound) ──────────────────────────────────────────────────────

#[tokio::test]
async fn drop_validator_unbound_removes() {
    let db = ShamirDb::init_memory().await.unwrap();

    let id = db
        .create_validator_from_wasm("v1", &echo_wasm(), false)
        .await
        .unwrap();

    let dropped = db.drop_validator("v1").await.unwrap();
    assert!(dropped, "drop should return true for existing validator");

    // Gone from registry.
    assert_eq!(db.validator_id("v1"), None);
    assert!(db.validators().get_by_id(&id).is_none());

    // Drop again → false.
    let dropped_again = db.drop_validator("v1").await.unwrap();
    assert!(
        !dropped_again,
        "second drop should return false (already gone)"
    );
}

#[tokio::test]
async fn drop_validator_persists_removal() {
    let db = ShamirDb::init_memory().await.unwrap();

    db.create_validator_from_wasm("v1", &echo_wasm(), false)
        .await
        .unwrap();
    db.drop_validator("v1").await.unwrap();

    // Catalogue should not contain the validator.
    let records = db.system_store().load_validators().await.unwrap();
    assert!(
        records.iter().all(|r| r["name"].as_str() != Some("v1")),
        "catalogue should not contain the dropped validator"
    );
}

// ── drop (bound) → error ────────────────────────────────────────────────

#[tokio::test]
async fn drop_validator_bound_refused() {
    let db = ShamirDb::init_memory().await.unwrap();

    let id = db
        .create_validator_from_wasm("v1", &echo_wasm(), false)
        .await
        .unwrap();

    // Simulate a binding (S2 will do this through DDL).
    db.validators().add_binding(&id, "testdb/main/users");

    // Drop must fail.
    let err = db.drop_validator("v1").await.unwrap_err();
    match err {
        DbError::Validation(msg) => {
            assert!(
                msg.contains("still bound"),
                "expected 'still bound', got: {msg}"
            );
            assert!(
                msg.contains("testdb/main/users"),
                "error should mention the bound table, got: {msg}"
            );
        }
        other => panic!("expected Validation error, got: {other}"),
    }

    // Validator must NOT have been removed.
    assert_eq!(db.validator_id("v1"), Some(id));
    assert!(db.validators().get_by_id(&id).is_some());
}

// ── binding helpers ─────────────────────────────────────────────────────

#[tokio::test]
async fn add_binding_and_is_bound() {
    let db = ShamirDb::init_memory().await.unwrap();

    let id = db
        .create_validator_from_wasm("v1", &echo_wasm(), false)
        .await
        .unwrap();

    assert!(!db.validators().is_bound(&id));

    db.validators().add_binding(&id, "db1/repo1/table1");
    assert!(db.validators().is_bound(&id));

    let tables = db.validators().bound_tables(&id);
    assert_eq!(tables, vec!["db1/repo1/table1".to_string()]);

    // Add another binding.
    db.validators().add_binding(&id, "db1/repo1/table2");
    let tables = db.validators().bound_tables(&id);
    assert_eq!(tables.len(), 2);

    // Remove one.
    db.validators().remove_binding(&id, "db1/repo1/table1");
    let tables = db.validators().bound_tables(&id);
    assert_eq!(tables, vec!["db1/repo1/table2".to_string()]);

    // Remove the last → unbound.
    db.validators().remove_binding(&id, "db1/repo1/table2");
    assert!(!db.validators().is_bound(&id));
}

// ── persistence (catalogue record exists) ───────────────────────────────

#[tokio::test]
async fn create_validator_persists_to_catalogue() {
    let db = ShamirDb::init_memory().await.unwrap();

    let id = db
        .create_validator_from_wasm("v1", &echo_wasm(), false)
        .await
        .unwrap();

    let records = db.system_store().load_validators().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["name"].as_str(), Some("v1"));
    assert_eq!(records[0]["_id"].as_str(), Some(id.to_string().as_str()));
    assert!(records[0]["wasm_b64"].as_str().is_some());
}

/// Open a redb-backed `ShamirDb`, tolerating the brief window where a
/// just-dropped previous instance's redb file lock is still being released.
async fn init_redb_retry(path: &std::path::Path) -> ShamirDb {
    for _ in 0..50 {
        match ShamirDb::init(SystemStoreConfig::Fjall(path.to_path_buf())).await {
            Ok(db) => return db,
            Err(e)
                if {
                    let m = e.to_string();
                    m.contains("Cannot acquire lock") || m.contains("already open")
                } =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => panic!("unexpected reopen error: {e}"),
        }
    }
    panic!("redb lock was not released within the retry window");
}

#[tokio::test]
async fn validators_persist_across_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("system.db");

    let id;
    // Open, create validator, close.
    {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(path.clone()))
            .await
            .unwrap();
        id = db
            .create_validator_from_wasm("v1", &echo_wasm(), false)
            .await
            .unwrap();

        // Verify it's live.
        assert!(db.validators().get_by_id(&id).is_some());
    }

    // Re-open — validator should be reloaded from catalogue.
    {
        let db = init_redb_retry(&path).await;
        assert_eq!(
            db.validator_id("v1"),
            Some(id),
            "validator name should resolve to the same id after reopen"
        );
        assert!(
            db.validators().get_by_id(&id).is_some(),
            "compiled validator should be present after reopen"
        );
    }
}

// ========================================================================
// S2 — per-table binding tests
// ========================================================================

/// Helper: create an in-memory ShamirDb with "testdb/main/users" table
/// and a validator "v1".
async fn setup_with_table() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo("testdb", repo_config).await.unwrap();
    db.create_validator_from_wasm("v1", &echo_wasm(), false)
        .await
        .unwrap();
    db
}

#[tokio::test]
async fn bind_validator_shows_in_list() {
    let db = setup_with_table().await;

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v1",
        vec![WriteOp::Insert, WriteOp::Update],
        1500,
    )
    .await
    .unwrap();

    let bindings = db
        .list_validator_bindings("testdb", "main", "users")
        .await
        .unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].priority, 1500);
    assert_eq!(
        bindings[0].ops.as_slice(),
        &[WriteOp::Insert, WriteOp::Update]
    );

    // is_bound should be true in the global registry.
    let id = db.validator_id("v1").unwrap();
    assert!(db.validators().is_bound(&id));
}

#[tokio::test]
async fn bind_validator_drop_refused() {
    let db = setup_with_table().await;

    db.bind_validator("testdb", "main", "users", "v1", vec![WriteOp::Insert], 2000)
        .await
        .unwrap();

    // Drop should be refused because the validator is bound.
    let err = db.drop_validator("v1").await.unwrap_err();
    match err {
        DbError::Validation(msg) => {
            assert!(
                msg.contains("still bound"),
                "expected 'still bound', got: {msg}"
            );
            assert!(
                msg.contains("testdb/main/users"),
                "should mention the bound table, got: {msg}"
            );
        }
        other => panic!("expected Validation error, got: {other}"),
    }
}

#[tokio::test]
async fn bind_idempotent_updates_ops_and_priority() {
    let db = setup_with_table().await;

    // First bind.
    db.bind_validator("testdb", "main", "users", "v1", vec![WriteOp::Insert], 1000)
        .await
        .unwrap();

    // Re-bind with different ops and priority.
    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v1",
        vec![WriteOp::Update, WriteOp::Delete],
        5000,
    )
    .await
    .unwrap();

    let bindings = db
        .list_validator_bindings("testdb", "main", "users")
        .await
        .unwrap();
    assert_eq!(bindings.len(), 1, "idempotent bind should not duplicate");
    assert_eq!(bindings[0].priority, 5000);
    assert_eq!(
        bindings[0].ops.as_slice(),
        &[WriteOp::Update, WriteOp::Delete]
    );
}

#[tokio::test]
async fn unbind_validator_removes_binding() {
    let db = setup_with_table().await;

    db.bind_validator("testdb", "main", "users", "v1", vec![WriteOp::Insert], 1000)
        .await
        .unwrap();

    let removed = db
        .unbind_validator("testdb", "main", "users", "v1")
        .await
        .unwrap();
    assert!(removed, "unbind should return true when binding existed");

    // Binding list should be empty.
    let bindings = db
        .list_validator_bindings("testdb", "main", "users")
        .await
        .unwrap();
    assert!(bindings.is_empty());

    // is_bound should be false.
    let id = db.validator_id("v1").unwrap();
    assert!(!db.validators().is_bound(&id));

    // Drop should now succeed.
    let dropped = db.drop_validator("v1").await.unwrap();
    assert!(dropped);
}

#[tokio::test]
async fn unbind_nonexistent_returns_false() {
    let db = setup_with_table().await;

    let removed = db
        .unbind_validator("testdb", "main", "users", "v1")
        .await
        .unwrap();
    assert!(
        !removed,
        "unbind should return false when no binding existed"
    );
}

#[tokio::test]
async fn bind_priority_out_of_range_rejected() {
    let db = setup_with_table().await;

    // priority too low
    let err = db
        .bind_validator("testdb", "main", "users", "v1", vec![WriteOp::Insert], 999)
        .await
        .unwrap_err();
    match err {
        DbError::Validation(msg) => {
            assert!(
                msg.contains("priority"),
                "expected priority error, got: {msg}"
            );
        }
        other => panic!("expected Validation error, got: {other}"),
    }

    // priority too high
    let err = db
        .bind_validator(
            "testdb",
            "main",
            "users",
            "v1",
            vec![WriteOp::Insert],
            10000,
        )
        .await
        .unwrap_err();
    match err {
        DbError::Validation(msg) => {
            assert!(
                msg.contains("priority"),
                "expected priority error, got: {msg}"
            );
        }
        other => panic!("expected Validation error, got: {other}"),
    }
}

#[tokio::test]
async fn bind_empty_ops_rejected() {
    let db = setup_with_table().await;

    let err = db
        .bind_validator("testdb", "main", "users", "v1", vec![], 1500)
        .await
        .unwrap_err();
    match err {
        DbError::Validation(msg) => {
            assert!(
                msg.contains("non-empty"),
                "expected non-empty ops error, got: {msg}"
            );
        }
        other => panic!("expected Validation error, got: {other}"),
    }
}

#[tokio::test]
async fn bind_unknown_validator_rejected() {
    let db = setup_with_table().await;

    let err = db
        .bind_validator(
            "testdb",
            "main",
            "users",
            "nonexistent",
            vec![WriteOp::Insert],
            1500,
        )
        .await
        .unwrap_err();
    match err {
        DbError::Validation(msg) => {
            assert!(
                msg.contains("not found"),
                "expected 'not found' error, got: {msg}"
            );
        }
        other => panic!("expected Validation error, got: {other}"),
    }
}

#[tokio::test]
async fn binding_persists_in_info_twin() {
    let db = setup_with_table().await;

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v1",
        vec![WriteOp::Insert, WriteOp::Update],
        2500,
    )
    .await
    .unwrap();

    // Reload the table info-twin directly and verify the binding
    // round-trips. We access the TableManager's info_store and call
    // the persistence loader.
    let table = db.get_table("testdb", "main", "users").await.unwrap();
    let loaded =
        shamir_engine::validator::persistence::load_validators_metadata(table.info_store())
            .await
            .unwrap()
            .expect("should have persisted bindings");

    assert_eq!(loaded.bindings.len(), 1);
    let id = db.validator_id("v1").unwrap();
    assert_eq!(loaded.bindings[0].validator_id, id);
    assert_eq!(loaded.bindings[0].priority, 2500);
    assert_eq!(
        loaded.bindings[0].ops.as_slice(),
        &[WriteOp::Insert, WriteOp::Update]
    );
}
