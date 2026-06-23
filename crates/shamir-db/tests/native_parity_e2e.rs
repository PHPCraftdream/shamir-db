//! Phase 1 + Phase 4 — Native↔WASM parity integration tests.
//!
//! Phase 1 covers:
//! 1. `register_fn` + `invoke_function` round-trip (in-memory native fn).
//! 2. `register_native_validator` (NO `replace_artifact`) → bind → violating
//!    write rejected, valid write accepted.
//! 3. BOOT fail-closed: persist a `kind=Native` validator row, reopen WITHOUT
//!    re-registering → boot succeeds, write to the bound table fails closed.
//!
//! Phase 4 adds:
//! 4. `list_*_with_kind` reports `kind=Native` for native artifacts and
//!    `kind=Wasm` for wasm ones (catalogue honesty).
//! 5. `drop_validator` refuses a bound native validator (drop parity with
//!    wasm).
//! 6. `unresolved_native_artifacts()` returns the names of native catalogue
//!    rows whose live artifact was NOT re-registered after a reopen.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::validator::WriteOp;
use shamir_db::query::batch::BatchRequest;
use shamir_db::shamir_db::shamir_db::ArtifactKind;
use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_engine::function::{FnBatch, FnCtx, Params};
use shamir_engine::validator::{RecordFields, Validation, ValidatorCtx};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;
use shamir_types::mpack;
use shamir_types::record_view::ScalarRef;
use shamir_types::types::value::QueryValue;

// ========================================================================
// Helpers
// ========================================================================

/// In-memory ShamirDb with `testdb/main/users` table.
async fn setup_db() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo("testdb", repo_config).await.unwrap();
    db
}

fn insert_request(id: &str, record: QueryValue) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.insert("ins", insert("users").row(record));
    b.to_request_via_msgpack()
}

fn read_all_request(id: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.query("all", Query::from("users"));
    b.to_request_via_msgpack()
}

// ========================================================================
// Test 1: register_fn + invoke_function round-trip
// ========================================================================

#[tokio::test]
async fn register_fn_round_trip() {
    let db = setup_db().await;

    // Register a native function that echoes back its "text" parameter
    // uppercased.
    db.register_fn(
        "shout",
        false,
        |_ctx: &FnCtx, _batch: &FnBatch, params: &Params| {
            let text = params.str("text").unwrap().to_owned();
            async move { Ok(QueryValue::Str(text.to_uppercase())) }
        },
    )
    .unwrap();

    // Invoke it.
    let mut params = Params::new();
    params.set("text", QueryValue::Str("hello".into()));
    let result = db.invoke_function("shout", params).await.unwrap();
    assert_eq!(result, QueryValue::Str("HELLO".into()));
}

#[tokio::test]
async fn register_fn_replace_overwrites() {
    let db = setup_db().await;

    // First version: returns "v1".
    db.register_fn("versioned", false, |_ctx, _batch, _params| async move {
        Ok(QueryValue::Str("v1".into()))
    })
    .unwrap();

    let p = Params::new();
    let r1 = db.invoke_function("versioned", p.clone()).await.unwrap();
    assert_eq!(r1, QueryValue::Str("v1".into()));

    // Replace with v2.
    db.register_fn("versioned", true, |_ctx, _batch, _params| async move {
        Ok(QueryValue::Str("v2".into()))
    })
    .unwrap();

    let r2 = db.invoke_function("versioned", p).await.unwrap();
    assert_eq!(r2, QueryValue::Str("v2".into()));
}

#[tokio::test]
async fn register_fn_duplicate_errors() {
    let db = setup_db().await;

    db.register_fn("once", false, |_ctx, _batch, _params| async move {
        Ok(QueryValue::Null)
    })
    .unwrap();

    let err = db
        .register_fn("once", false, |_ctx, _batch, _params| async move {
            Ok(QueryValue::Null)
        })
        .unwrap_err();
    assert!(
        err.to_string().contains("already"),
        "expected 'already' error, got: {err}"
    );
}

// ========================================================================
// Test 2: register_native_validator — NO replace_artifact needed
// ========================================================================

#[tokio::test]
async fn native_validator_accepts_valid_write() {
    let db = setup_db().await;

    // A native validator that rejects records where "age" < 18.
    db.register_native_validator(
        "check_age",
        false,
        |new: Option<&dyn RecordFields>,
         _prev: Option<&dyn RecordFields>,
         _ctx: &ValidatorCtx<'_>| {
            let age = new.and_then(|f| f.scalar(&["age"])).and_then(|s| {
                if let ScalarRef::Int(i) = s {
                    Some(i)
                } else {
                    None
                }
            });
            match age {
                Some(a) if a < 18 => {
                    let mut v = Validation::accept();
                    v.field_error(vec!["age".into()], "too_young");
                    v
                }
                _ => Validation::accept(),
            }
        },
    )
    .await
    .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "check_age",
        vec![WriteOp::Insert],
        1500,
    )
    .await
    .unwrap();

    // Valid write (age >= 18) → accepted.
    let req = insert_request("ok", mpack!({"name": "Alice", "age": 30}));
    let resp = db.execute("testdb", &req).await;
    assert!(resp.is_ok(), "valid write should succeed: {:?}", resp.err());

    // Verify persisted.
    let read = read_all_request("verify");
    let read_resp = db.execute("testdb", &read).await.unwrap();
    assert_eq!(read_resp.results["all"].records.len(), 1);
}

#[tokio::test]
async fn native_validator_rejects_violating_write() {
    let db = setup_db().await;

    db.register_native_validator(
        "check_age",
        false,
        |new: Option<&dyn RecordFields>,
         _prev: Option<&dyn RecordFields>,
         _ctx: &ValidatorCtx<'_>| {
            let age = new.and_then(|f| f.scalar(&["age"])).and_then(|s| {
                if let ScalarRef::Int(i) = s {
                    Some(i)
                } else {
                    None
                }
            });
            match age {
                Some(a) if a < 18 => {
                    let mut v = Validation::accept();
                    v.field_error(vec!["age".into()], "too_young");
                    v
                }
                _ => Validation::accept(),
            }
        },
    )
    .await
    .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "check_age",
        vec![WriteOp::Insert],
        1500,
    )
    .await
    .unwrap();

    // Violating write (age < 18) → rejected.
    let req = insert_request("bad", mpack!({"name": "Bob", "age": 12}));
    let resp = db.execute("testdb", &req).await;
    assert!(resp.is_err(), "violating write should be rejected");

    let err_msg = resp.unwrap_err().to_string();
    assert!(
        err_msg.contains("too_young"),
        "error should contain 'too_young', got: {err_msg}"
    );

    // Verify NOT persisted.
    let read = read_all_request("verify_empty");
    let read_resp = db.execute("testdb", &read).await.unwrap();
    assert!(
        read_resp.results["all"].records.is_empty(),
        "rejected row should NOT be persisted"
    );
}

#[tokio::test]
async fn native_validator_with_stop_flag() {
    let db = setup_db().await;

    // A validator that always rejects with stop=true.
    db.register_native_validator(
        "stop_all",
        false,
        |_new: Option<&dyn RecordFields>,
         _prev: Option<&dyn RecordFields>,
         _ctx: &ValidatorCtx<'_>| {
            let mut v = Validation::reject("blocked");
            v.stop();
            v
        },
    )
    .await
    .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "stop_all",
        vec![WriteOp::Insert],
        1000,
    )
    .await
    .unwrap();

    let req = insert_request("blocked", mpack!({"name": "X"}));
    let resp = db.execute("testdb", &req).await;
    assert!(resp.is_err());
    assert!(resp.unwrap_err().to_string().contains("blocked"));
}

// ========================================================================
// Test 3: BOOT fail-closed — Native row tolerated, write fails closed
// ========================================================================

/// Open a Fjall-backed ShamirDb, tolerating lock-release delays.
async fn init_fjall_retry(path: &std::path::Path) -> ShamirDb {
    for _ in 0..50 {
        match ShamirDb::init(SystemStoreConfig::Fjall(path.to_path_buf())).await {
            Ok(db) => return db,
            Err(e)
                if {
                    let m = e.to_string();
                    m.contains("Cannot acquire lock")
                        || m.contains("already open")
                        || m.contains("Locked")
                } =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => panic!("unexpected reopen error: {e}"),
        }
    }
    panic!("lock was not released within the retry window");
}

#[tokio::test]
async fn boot_tolerates_native_validator_row() {
    let tmp = tempfile::tempdir().unwrap();
    let sys_path = tmp.path().join("system.db");
    let repo_path = tmp.path().join("repo_main");

    let id;
    {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        db.create_db("testdb").await;

        // Use a Fjall-backed repo so table info-twin (bindings) persist.
        let repo_config = RepoConfig::new("main", BoxRepoFactory::fjall(repo_path.clone()))
            .add_table(TableConfig::new("users"));
        db.add_repo("testdb", repo_config).await.unwrap();

        // Register a native validator — persists a kind=Native row with no wasm_b64.
        id = db
            .register_native_validator(
                "v_native",
                false,
                |_new: Option<&dyn RecordFields>,
                 _prev: Option<&dyn RecordFields>,
                 _ctx: &ValidatorCtx<'_>| Validation::accept(),
            )
            .await
            .unwrap();

        // Bind it to the table.
        db.bind_validator(
            "testdb",
            "main",
            "users",
            "v_native",
            vec![WriteOp::Insert],
            1500,
        )
        .await
        .unwrap();

        // Verify the validator is live.
        assert!(db.validators().get_by_id(&id).is_some());
    }

    // Re-open WITHOUT re-registering the native artifact.
    {
        let db = init_fjall_retry(&sys_path).await;

        // Boot must succeed (not crash on the Native row).
        // The validator catalogue row exists but the artifact is NOT registered.
        let records = db.system_store().load_validators().await.unwrap();
        assert_eq!(records.len(), 1, "catalogue row should be persisted");
        assert_eq!(records[0]["name"].as_str(), Some("v_native"));

        // The artifact must NOT be registered (boot skipped it).
        assert!(
            db.validators().get_by_id(&id).is_none(),
            "native artifact must NOT be registered after boot (embedder must re-register)"
        );

        // Attempt a write to the bound table — must fail closed, NOT panic.
        let req = insert_request("fail_closed", mpack!({"name": "Test"}));
        let resp = db.execute("testdb", &req).await;
        assert!(
            resp.is_err(),
            "write should fail closed when native artifact is not re-registered"
        );
        let err_msg = resp.unwrap_err().to_string();
        // The error should indicate the validator was not found / missing.
        assert!(
            err_msg.contains("not found in registry")
                || err_msg.contains("Validator invalid")
                || err_msg.contains("missing"),
            "error should mention the missing validator, got: {err_msg}"
        );
    }
}

#[tokio::test]
async fn boot_native_validator_re_register_works() {
    let tmp = tempfile::tempdir().unwrap();
    let sys_path = tmp.path().join("system.db");
    let repo_path = tmp.path().join("repo_main");

    {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        db.create_db("testdb").await;
        let repo_config = RepoConfig::new("main", BoxRepoFactory::fjall(repo_path.clone()))
            .add_table(TableConfig::new("users"));
        db.add_repo("testdb", repo_config).await.unwrap();

        db.register_native_validator(
            "v_native2",
            false,
            |_new: Option<&dyn RecordFields>,
             _prev: Option<&dyn RecordFields>,
             _ctx: &ValidatorCtx<'_>| Validation::accept(),
        )
        .await
        .unwrap();

        db.bind_validator(
            "testdb",
            "main",
            "users",
            "v_native2",
            vec![WriteOp::Insert],
            1500,
        )
        .await
        .unwrap();
    }

    // Re-open and RE-REGISTER the native artifact.
    {
        let db = init_fjall_retry(&sys_path).await;

        // Re-register: this must work because the catalogue row is replaceable.
        db.register_native_validator(
            "v_native2",
            true,
            |_new: Option<&dyn RecordFields>,
             _prev: Option<&dyn RecordFields>,
             _ctx: &ValidatorCtx<'_>| Validation::accept(),
        )
        .await
        .unwrap();

        // Now the artifact is registered — write should succeed.
        let req = insert_request("ok_after_reregister", mpack!({"name": "Works"}));
        let resp = db.execute("testdb", &req).await;
        assert!(
            resp.is_ok(),
            "write should succeed after re-registering the native artifact: {:?}",
            resp.err()
        );
    }
}

// ========================================================================
// Phase 4 — catalogue honesty: kind surfaced in list/show
// ========================================================================

/// Minimal WASM bytes: a module whose `shamir_call` returns msgpack `null`.
fn accept_wasm_bytes() -> Vec<u8> {
    wat::parse_str(
        r#"
(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (data (i32.const 512) "\\c0")
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or (i64.shl (i64.const 512) (i64.const 32)) (i64.const 1))))
"#,
    )
    .expect("WAT parse failed")
}

#[tokio::test]
async fn list_functions_with_kind_reports_native_and_wasm() {
    let db = setup_db().await;

    // A native function registered via `register_fn` — in-memory only, no
    // catalogue row. From the catalogue's perspective it has no `kind`
    // evidence, so `list_functions_with_kind` defaults it to Wasm (the
    // historical default for any row without an explicit `kind` field).
    db.register_fn(
        "ephemeral_native",
        false,
        |_ctx, _batch, _params| async move { Ok(QueryValue::Null) },
    )
    .unwrap();

    // A wasm function — persisted to catalogue with kind = Wasm.
    db.create_function_from_wasm("wasm_fn", &accept_wasm_bytes(), false)
        .await
        .unwrap();

    // A native function with a MANUALLY persisted kind = Native catalogue
    // row (the documented path for a persisted native function — see the
    // `register_fn` doc-comment). This is the case where the catalogue
    // honestly reports Native.
    db.register_fn(
        "persisted_native",
        false,
        |_ctx, _batch, _params| async move { Ok(QueryValue::Null) },
    )
    .unwrap();
    {
        use shamir_db::access::ResourceMeta;
        use shamir_db::shamir_db::shamir_db::KIND_FIELD;
        use shamir_types::types::common::new_map;
        let mut m = new_map();
        m.insert(
            "name".to_string(),
            QueryValue::Str("persisted_native".to_string()),
        );
        m.insert("lang".to_string(), QueryValue::Str("rust".to_string()));
        m.insert(
            KIND_FIELD.to_string(),
            ArtifactKind::Native.as_query_value(),
        );
        let rec = QueryValue::Map(m);
        db.system_store()
            .save_function("persisted_native", &rec, &ResourceMeta::open())
            .await
            .unwrap();
    }

    let entries = db.list_functions_with_kind().await.unwrap();
    let kind_of = |target: &str| -> ArtifactKind {
        entries
            .iter()
            .find(|(n, _)| n == target)
            .map(|(_, k)| *k)
            .unwrap_or_else(|| panic!("function '{target}' should be listed"))
    };

    // Ephemeral native fn (no catalogue row) → defaults to Wasm.
    assert_eq!(
        kind_of("ephemeral_native"),
        ArtifactKind::Wasm,
        "in-memory-only native fn (no catalogue row) defaults to Wasm"
    );

    // Persisted wasm fn → Wasm.
    assert_eq!(
        kind_of("wasm_fn"),
        ArtifactKind::Wasm,
        "wasm function should report kind = Wasm"
    );

    // Persisted native fn (catalogue row with kind = Native) → Native.
    assert_eq!(
        kind_of("persisted_native"),
        ArtifactKind::Native,
        "persisted native function (catalogue kind = Native) should report kind = Native"
    );
}

#[tokio::test]
async fn list_validators_with_kind_reports_native_and_wasm() {
    let db = setup_db().await;

    // A native validator — persisted to catalogue with kind = Native.
    db.register_native_validator(
        "native_val",
        false,
        |_new: Option<&dyn RecordFields>,
         _prev: Option<&dyn RecordFields>,
         _ctx: &ValidatorCtx<'_>| { Validation::accept() },
    )
    .await
    .unwrap();

    // A wasm validator — persisted to catalogue with kind = Wasm.
    db.create_validator_from_wasm("wasm_val", &accept_wasm_bytes(), false)
        .await
        .unwrap();

    let entries = db.list_validators_with_kind().await.unwrap();
    let kind_of = |target: &str| -> ArtifactKind {
        entries
            .iter()
            .find(|(_, n, _)| n == target)
            .map(|(_, _, k)| *k)
            .unwrap_or_else(|| panic!("validator '{target}' should be listed"))
    };

    assert_eq!(
        kind_of("native_val"),
        ArtifactKind::Native,
        "native validator must report kind = Native"
    );
    assert_eq!(
        kind_of("wasm_val"),
        ArtifactKind::Wasm,
        "wasm validator must report kind = Wasm"
    );
}

// ========================================================================
// Phase 4 — drop parity: bound native validator refused
// ========================================================================

#[tokio::test]
async fn drop_refuses_bound_native_validator() {
    let db = setup_db().await;

    // Register a native validator.
    db.register_native_validator(
        "bound_native",
        false,
        |_new: Option<&dyn RecordFields>,
         _prev: Option<&dyn RecordFields>,
         _ctx: &ValidatorCtx<'_>| { Validation::accept() },
    )
    .await
    .unwrap();

    // Bind it to the users table.
    db.bind_validator(
        "testdb",
        "main",
        "users",
        "bound_native",
        vec![WriteOp::Insert],
        1500,
    )
    .await
    .unwrap();

    // Dropping while bound must be refused — same semantics as a wasm
    // validator. The drop path is kind-agnostic (it goes through
    // validators.id_for_name + is_bound, both keyed by RecordId).
    let err = db.drop_validator("bound_native").await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("still bound") || msg.contains("bound to tables"),
        "drop of bound native validator should be refused, got: {msg}"
    );

    // The validator must still be registered (drop was refused).
    assert!(
        db.validators().id_for_name("bound_native").is_some(),
        "bound native validator must still be registered after refused drop"
    );

    // After unbinding, the drop succeeds — parity with wasm.
    db.unbind_validator("testdb", "main", "users", "bound_native")
        .await
        .unwrap();
    let dropped = db.drop_validator("bound_native").await.unwrap();
    assert!(dropped, "drop should succeed after unbinding");
    assert!(
        db.validators().id_for_name("bound_native").is_none(),
        "validator must be gone after successful drop"
    );
}

// ========================================================================
// Phase 4 — boot diagnostic: unresolved native artifacts surfaced
// ========================================================================

#[tokio::test]
async fn unresolved_native_artifacts_lists_unregistered_after_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let sys_path = tmp.path().join("system.db");
    let repo_path = tmp.path().join("repo_main");

    {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        db.create_db("testdb").await;
        let repo_config = RepoConfig::new("main", BoxRepoFactory::fjall(repo_path.clone()))
            .add_table(TableConfig::new("users"));
        db.add_repo("testdb", repo_config).await.unwrap();

        // Persist a native validator row (kind = Native, no wasm_b64).
        db.register_native_validator(
            "orphan_native_val",
            false,
            |_new: Option<&dyn RecordFields>,
             _prev: Option<&dyn RecordFields>,
             _ctx: &ValidatorCtx<'_>| { Validation::accept() },
        )
        .await
        .unwrap();

        // While live, unresolved_native_artifacts is empty — the artifact
        // is registered.
        assert!(
            db.unresolved_native_artifacts().await.unwrap().is_empty(),
            "no unresolved artifacts while the native validator is live"
        );
    }

    // Reopen WITHOUT re-registering the native artifact.
    {
        let db = init_fjall_retry(&sys_path).await;

        let unresolved = db.unresolved_native_artifacts().await.unwrap();
        assert_eq!(
            unresolved,
            vec!["orphan_native_val".to_string()],
            "reopened DB should report the un-re-registered native validator"
        );

        // The boot warn was emitted (visible in test log output). Re-registering
        // clears the diagnostic.
        db.register_native_validator(
            "orphan_native_val",
            true,
            |_new: Option<&dyn RecordFields>,
             _prev: Option<&dyn RecordFields>,
             _ctx: &ValidatorCtx<'_>| { Validation::accept() },
        )
        .await
        .unwrap();
        let unresolved_after = db.unresolved_native_artifacts().await.unwrap();
        assert!(
            unresolved_after.is_empty(),
            "after re-registering, no unresolved artifacts should remain: {:?}",
            unresolved_after
        );
    }
}
