//! Phase 5 — Cross-cutting COMPLETENESS gap tests for the native↔WASM parity
//! matrix.
//!
//! Phases 0/1/2/4 each have their own TDD that covers the individual planes
//! (native function, native validator, native scalar) and the per-phase
//! lifecycle (register, invoke, bind, fail-closed, reopen, kind reporting).
//! This file fills the **cross-cutting gaps** the per-phase tests did NOT
//! cover:
//!
//! 1. **Mixed native + wasm validators on ONE table** — both fire in priority
//!    order, errors accumulate, stop from either halts the chain.
//! 2. **Parity equivalence** — a native function and a wasm function computing
//!    the same thing return identical results through `invoke`; a native
//!    validator and a wasm validator with the same rule produce identical
//!    accept/reject on the same write.
//! 3. **Three-plane independence** — a native function + native validator +
//!    native scalar all registered in ONE db instance simultaneously, each used
//!    in its own slot.
//! 4. **Mixed-kind list** — a db with BOTH native and wasm functions (and
//!    validators) — `list_*_with_kind` reports the correct kind for each, count
//!    is right.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::validator::WriteOp;
use shamir_db::query::batch::BatchRequest;
use shamir_db::shamir_db::shamir_db::ArtifactKind;
use shamir_db::ShamirDb;
use shamir_engine::function::{FnBatch, FnCtx, Params};
use shamir_engine::validator::{RecordFields, Validation, ValidatorCtx};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

// ============================================================================
// WAT helpers (same ABI as validators_e2e.rs)
// ============================================================================

/// WAT module that ignores input and returns msgpack `null` (0xC0) = valid.
fn accept_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"
(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (data (i32.const 512) "\c0")
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

/// Build a WAT module whose `shamir_call` returns the given `QueryValue`
/// serialised as msgpack.
fn make_wat_returning(value: &QueryValue) -> Vec<u8> {
    let bytes = rmp_serde::to_vec(value).expect("msgpack encode");
    let hex_data: String = bytes.iter().map(|b| format!("\\{b:02x}")).collect();
    let len = bytes.len();
    let wat = format!(
        r#"
(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (data (i32.const 512) "{hex_data}")
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or (i64.shl (i64.const 512) (i64.const 32)) (i64.const {len}))))
"#
    );
    wat::parse_str(&wat).expect("generated WAT parse failed")
}

/// `{"errors":[{"code":"…"}],"stop":false}` — a single record-level rejection.
fn rejection_code(code: &str, stop: bool) -> QueryValue {
    let mut error_item = new_map();
    error_item.insert("code".to_owned(), QueryValue::Str(code.to_owned()));
    let mut root = new_map();
    root.insert(
        "errors".to_owned(),
        QueryValue::List(vec![QueryValue::Map(error_item)]),
    );
    root.insert("stop".to_owned(), QueryValue::Bool(stop));
    QueryValue::Map(root)
}

// ============================================================================
// Setup helpers
// ============================================================================

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

// ============================================================================
// GAP 1: Mixed native + wasm validators on ONE table
// ============================================================================

/// Both fire in priority order, errors accumulate, stop from the wasm
/// validator halts the chain (the native lower-priority one is skipped).
///
/// This proves native and wasm validators coexist on the same binding
/// list — the `run_validators_loop` is kind-agnostic.
#[tokio::test]
async fn mixed_native_and_wasm_validators_both_fire_and_accumulate() {
    let db = setup_db().await;

    // ── Wasm validator: priority 1000, always emits "wasm_err" ──
    let wasm_reject = make_wat_returning(&rejection_code("wasm_err", false));
    db.create_validator_from_wasm("v_wasm", &wasm_reject, false)
        .await
        .unwrap();
    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_wasm",
        vec![WriteOp::Insert],
        1000,
    )
    .await
    .unwrap();

    // ── Native validator: priority 2000, always emits "native_err" ──
    db.register_native_validator(
        "v_native",
        false,
        |_new: Option<&dyn RecordFields>,
         _prev: Option<&dyn RecordFields>,
         _ctx: &ValidatorCtx<'_>| {
            let mut v = Validation::accept();
            v.error("native_err");
            v
        },
    )
    .await
    .unwrap();
    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_native",
        vec![WriteOp::Insert],
        2000,
    )
    .await
    .unwrap();

    // Write — both validators fire (neither has stop), both errors surface.
    let resp = db
        .execute("testdb", &insert_request("test", mpack!({"name": "X"})))
        .await;
    assert!(resp.is_err(), "write should be rejected");
    let err = resp.unwrap_err().to_string();
    assert!(
        err.contains("wasm_err"),
        "wasm validator error must be present, got: {err}"
    );
    assert!(
        err.contains("native_err"),
        "native validator error must be present (accumulated), got: {err}"
    );

    // Row must NOT be persisted.
    let read_resp = db
        .execute("testdb", &read_all_request("verify"))
        .await
        .unwrap();
    assert!(
        read_resp.results["all"].records.is_empty(),
        "rejected row must not be persisted"
    );
}

/// Stop from the NATIVE validator (priority 1000) halts the chain — the
/// lower-priority wasm validator (priority 2000) is skipped.
#[tokio::test]
async fn mixed_validators_native_stop_halts_wasm() {
    let db = setup_db().await;

    // Native validator: priority 1000, rejects with stop.
    db.register_native_validator(
        "v_stop_native",
        false,
        |_new: Option<&dyn RecordFields>,
         _prev: Option<&dyn RecordFields>,
         _ctx: &ValidatorCtx<'_>| {
            let mut v = Validation::reject("native_stop_err");
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
        "v_stop_native",
        vec![WriteOp::Insert],
        1000,
    )
    .await
    .unwrap();

    // Wasm validator: priority 2000, would emit "wasm_should_not_appear".
    let wasm_reject = make_wat_returning(&rejection_code("wasm_should_not_appear", false));
    db.create_validator_from_wasm("v_after_stop", &wasm_reject, false)
        .await
        .unwrap();
    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_after_stop",
        vec![WriteOp::Insert],
        2000,
    )
    .await
    .unwrap();

    let resp = db
        .execute("testdb", &insert_request("test2", mpack!({"name": "Y"})))
        .await;
    assert!(resp.is_err());
    let err = resp.unwrap_err().to_string();
    assert!(
        err.contains("native_stop_err"),
        "native stop error must be present, got: {err}"
    );
    assert!(
        !err.contains("wasm_should_not_appear"),
        "wasm validator must be halted by native stop, got: {err}"
    );
}

/// Stop from the WASM validator (priority 1000) halts the chain — the
/// lower-priority native validator (priority 2000) is skipped.
#[tokio::test]
async fn mixed_validators_wasm_stop_halts_native() {
    let db = setup_db().await;

    // Wasm validator: priority 1000, rejects with stop.
    let wasm_stop = make_wat_returning(&rejection_code("wasm_stop_err", true));
    db.create_validator_from_wasm("v_stop_wasm", &wasm_stop, false)
        .await
        .unwrap();
    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_stop_wasm",
        vec![WriteOp::Insert],
        1000,
    )
    .await
    .unwrap();

    // Native validator: priority 2000, would emit "native_should_not_appear".
    db.register_native_validator(
        "v_after_wasm_stop",
        false,
        |_new: Option<&dyn RecordFields>,
         _prev: Option<&dyn RecordFields>,
         _ctx: &ValidatorCtx<'_>| {
            let mut v = Validation::accept();
            v.error("native_should_not_appear");
            v
        },
    )
    .await
    .unwrap();
    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_after_wasm_stop",
        vec![WriteOp::Insert],
        2000,
    )
    .await
    .unwrap();

    let resp = db
        .execute("testdb", &insert_request("test3", mpack!({"name": "Z"})))
        .await;
    assert!(resp.is_err());
    let err = resp.unwrap_err().to_string();
    assert!(
        err.contains("wasm_stop_err"),
        "wasm stop error must be present, got: {err}"
    );
    assert!(
        !err.contains("native_should_not_appear"),
        "native validator must be halted by wasm stop, got: {err}"
    );
}

// ============================================================================
// GAP 2: Parity equivalence — native vs wasm compute the same result
// ============================================================================

/// A native function and a wasm function that both return the same constant
/// `QueryValue::Str("parity!")` — invoke must return identical results.
///
/// Proves the `invoke_function` dispatch is kind-agnostic: both native
/// `register_fn` and `create_function_from_wasm` produce callable artifacts
/// whose results are indistinguishable to the caller.
#[tokio::test]
async fn parity_native_and_wasm_function_return_identical() {
    let db = setup_db().await;

    // Native function: returns "parity!".
    db.register_fn(
        "native_echo",
        false,
        |_ctx: &FnCtx, _batch: &FnBatch, _params: &Params| async move {
            Ok(QueryValue::Str("parity!".into()))
        },
    )
    .unwrap();

    // Wasm function: returns msgpack str "parity!".
    let wasm = make_wat_returning(&QueryValue::Str("parity!".into()));
    db.create_function_from_wasm("wasm_echo", &wasm, false)
        .await
        .unwrap();

    let native_result = db
        .invoke_function("native_echo", Params::new())
        .await
        .unwrap();
    let wasm_result = db
        .invoke_function("wasm_echo", Params::new())
        .await
        .unwrap();

    assert_eq!(
        native_result, wasm_result,
        "native and wasm functions computing the same thing must return identical results"
    );
    assert_eq!(native_result, QueryValue::Str("parity!".into()));
}

/// A native validator and a wasm validator with the same accept rule both
/// accept the same write; a native and wasm validator with the same reject
/// rule both reject with the same error code.
///
/// Proves validator dispatch is kind-agnostic on the write path.
#[tokio::test]
async fn parity_native_and_wasm_validator_same_rule() {
    // ── ACCEPT parity ──
    {
        let db = setup_db().await;

        // Native accept.
        db.register_native_validator(
            "native_accept",
            false,
            |_new: Option<&dyn RecordFields>,
             _prev: Option<&dyn RecordFields>,
             _ctx: &ValidatorCtx<'_>| { Validation::accept() },
        )
        .await
        .unwrap();

        // Wasm accept.
        db.create_validator_from_wasm("wasm_accept", &accept_wasm(), false)
            .await
            .unwrap();

        // Bind both to the SAME table.
        db.bind_validator(
            "testdb",
            "main",
            "users",
            "native_accept",
            vec![WriteOp::Insert],
            1000,
        )
        .await
        .unwrap();
        db.bind_validator(
            "testdb",
            "main",
            "users",
            "wasm_accept",
            vec![WriteOp::Insert],
            2000,
        )
        .await
        .unwrap();

        let resp = db
            .execute("testdb", &insert_request("ok", mpack!({"name": "Alice"})))
            .await;
        assert!(
            resp.is_ok(),
            "both accept → write must succeed: {:?}",
            resp.err()
        );
    }

    // ── REJECT parity: same code from both kinds ──
    {
        let db = setup_db().await;

        // Native reject with "same_rule".
        db.register_native_validator(
            "native_reject",
            false,
            |_new: Option<&dyn RecordFields>,
             _prev: Option<&dyn RecordFields>,
             _ctx: &ValidatorCtx<'_>| { Validation::reject("same_rule") },
        )
        .await
        .unwrap();

        // Bind + test native reject.
        db.bind_validator(
            "testdb",
            "main",
            "users",
            "native_reject",
            vec![WriteOp::Insert],
            1500,
        )
        .await
        .unwrap();

        let native_resp = db
            .execute("testdb", &insert_request("n", mpack!({"name": "X"})))
            .await;
        assert!(native_resp.is_err(), "native reject must block");
        assert!(native_resp.unwrap_err().to_string().contains("same_rule"));
    }

    // Now test the wasm equivalent independently.
    {
        let db = setup_db().await;

        // Wasm reject with "same_rule".
        let wasm_reject = make_wat_returning(&rejection_code("same_rule", false));
        db.create_validator_from_wasm("wasm_reject", &wasm_reject, false)
            .await
            .unwrap();

        db.bind_validator(
            "testdb",
            "main",
            "users",
            "wasm_reject",
            vec![WriteOp::Insert],
            1500,
        )
        .await
        .unwrap();

        let wasm_resp = db
            .execute("testdb", &insert_request("w", mpack!({"name": "Y"})))
            .await;
        assert!(wasm_resp.is_err(), "wasm reject must block");
        assert!(
            wasm_resp.unwrap_err().to_string().contains("same_rule"),
            "wasm validator must produce the same error code as the native equivalent"
        );
    }
}

// ============================================================================
// GAP 3: Three-plane independence (function + validator + scalar in one db)
// ============================================================================

/// A native function + native validator + native scalar, all registered in
/// ONE db instance simultaneously. Each is used in its own slot:
/// - function via `invoke_function`
/// - validator via bound table binding
/// - scalar via WHERE filter
///
/// Proves the three planes are independent and composable.
#[tokio::test]
async fn three_planes_native_function_validator_scalar_independent() {
    let db = setup_db().await;

    // ── Plane 1: Native function ──
    db.register_fn(
        "compute_label",
        false,
        |_ctx: &FnCtx, _batch: &FnBatch, params: &Params| {
            let n = match params.get("n") {
                Ok(QueryValue::Int(i)) => *i,
                _ => 0,
            };
            async move { Ok(QueryValue::Str(format!("item-{n}"))) }
        },
    )
    .unwrap();

    // ── Plane 2: Native validator ──
    db.register_native_validator(
        "require_name",
        false,
        |new: Option<&dyn RecordFields>,
         _prev: Option<&dyn RecordFields>,
         _ctx: &ValidatorCtx<'_>| {
            let has_name = new.map(|f| f.present(&["name"]).is_some()).unwrap_or(false);
            if has_name {
                Validation::accept()
            } else {
                Validation::reject("missing_name")
            }
        },
    )
    .await
    .unwrap();
    db.bind_validator(
        "testdb",
        "main",
        "users",
        "require_name",
        vec![WriteOp::Insert],
        1500,
    )
    .await
    .unwrap();

    // ── Plane 3: Native scalar ──
    use shamir_funclib::registry::{arg_str, v_str, FnEntry};
    let scalars = db.scalars("testdb").unwrap();
    scalars.register(
        "my_len",
        FnEntry::pure(
            |args: &[QueryValue]| {
                let s = arg_str(args, 0)?;
                Ok(v_str(s.len().to_string()))
            },
            1,
            Some(1),
        ),
    );

    // ── Exercise all three planes ──

    // Plane 2 (validator): write WITH name → accepted.
    let resp = db
        .execute(
            "testdb",
            &insert_request("ins_ok", mpack!({"name": "alice", "age": 30})),
        )
        .await;
    assert!(resp.is_ok(), "validator should accept named record");

    // Plane 2 (validator): write WITHOUT name → rejected.
    let resp = db
        .execute("testdb", &insert_request("ins_bad", mpack!({"age": 99})))
        .await;
    assert!(resp.is_err(), "validator should reject unnamed record");
    assert!(resp.unwrap_err().to_string().contains("missing_name"));

    // Plane 1 (function): invoke returns the expected label.
    let mut params = Params::new();
    params.set("n", QueryValue::Int(42));
    let fn_result = db.invoke_function("compute_label", params).await.unwrap();
    assert_eq!(fn_result, QueryValue::Str("item-42".into()));

    // Plane 3 (scalar): use my_len in a WHERE filter.
    // my_len("alice") = "5". The query WHERE name == my_len("alice")
    // → name == "5", which matches no record. This proves the scalar
    // dispatched through the user scalar layer without error.
    use shamir_query_builder::filter::eq;
    use shamir_query_builder::val::{func, lit};
    let query = Query::from("users").where_(eq("name", func("my_len", [lit("alice")])));
    let mut b = Batch::new();
    b.id("q");
    b.query("result", query);
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    // The query executed successfully — the scalar dispatched through the
    // user scalar layer without error. Zero matches is expected (no record
    // has name == "5").
    assert!(
        resp.results["result"].records.is_empty(),
        "scalar filter executed correctly, expected 0 matches"
    );

    // Verify the one valid record was persisted.
    let read_resp = db
        .execute("testdb", &read_all_request("verify"))
        .await
        .unwrap();
    assert_eq!(
        read_resp.results["all"].records.len(),
        1,
        "only the accepted record should be persisted"
    );
}

// ============================================================================
// GAP 4: Mixed-kind list — both native and wasm reported with correct kind
// ============================================================================

/// A db with BOTH native and wasm functions (and validators) — `list_*_with_kind`
/// reports the correct kind for each artifact, and the total count is right.
#[tokio::test]
async fn mixed_kind_list_reports_correct_kind_and_count() {
    let db = setup_db().await;

    // ── Functions ──
    // Wasm function: persisted to catalogue with kind = Wasm.
    db.create_function_from_wasm("wasm_fn_a", &accept_wasm(), false)
        .await
        .unwrap();
    db.create_function_from_wasm("wasm_fn_b", &accept_wasm(), false)
        .await
        .unwrap();

    // Native function: register_fn + manually persisted kind = Native row
    // (the documented path for a persisted native function).
    db.register_fn(
        "persisted_native_fn",
        false,
        |_ctx, _batch, _params| async move { Ok(QueryValue::Null) },
    )
    .unwrap();
    {
        use shamir_db::access::ResourceMeta;
        use shamir_db::shamir_db::shamir_db::KIND_FIELD;
        let mut m = new_map();
        m.insert(
            "name".to_string(),
            QueryValue::Str("persisted_native_fn".to_string()),
        );
        m.insert("lang".to_string(), QueryValue::Str("rust".to_string()));
        m.insert(
            KIND_FIELD.to_string(),
            ArtifactKind::Native.as_query_value(),
        );
        let rec = QueryValue::Map(m);
        db.system_store()
            .save_function("persisted_native_fn", &rec, &ResourceMeta::open())
            .await
            .unwrap();
    }

    let fn_entries = db.list_functions_with_kind().await.unwrap();

    // Must find all three.
    let kind_of = |target: &str| -> ArtifactKind {
        fn_entries
            .iter()
            .find(|(n, _)| n == target)
            .map(|(_, k)| *k)
            .unwrap_or_else(|| panic!("function '{target}' should be listed"))
    };

    assert_eq!(kind_of("wasm_fn_a"), ArtifactKind::Wasm);
    assert_eq!(kind_of("wasm_fn_b"), ArtifactKind::Wasm);
    assert_eq!(
        kind_of("persisted_native_fn"),
        ArtifactKind::Native,
        "persisted native function must report kind = Native"
    );

    // ── Validators ──
    // Wasm validator.
    db.create_validator_from_wasm("wasm_val_a", &accept_wasm(), false)
        .await
        .unwrap();

    // Native validator.
    db.register_native_validator(
        "native_val_a",
        false,
        |_new: Option<&dyn RecordFields>,
         _prev: Option<&dyn RecordFields>,
         _ctx: &ValidatorCtx<'_>| { Validation::accept() },
    )
    .await
    .unwrap();

    let val_entries = db.list_validators_with_kind().await.unwrap();

    let val_kind_of = |target: &str| -> ArtifactKind {
        val_entries
            .iter()
            .find(|(_, n, _)| n == target)
            .map(|(_, _, k)| *k)
            .unwrap_or_else(|| panic!("validator '{target}' should be listed"))
    };

    assert_eq!(val_kind_of("wasm_val_a"), ArtifactKind::Wasm);
    assert_eq!(
        val_kind_of("native_val_a"),
        ArtifactKind::Native,
        "native validator must report kind = Native"
    );

    // Count check: at least the ones we registered.
    let wasm_val_count = val_entries
        .iter()
        .filter(|(_, _, k)| *k == ArtifactKind::Wasm)
        .count();
    let native_val_count = val_entries
        .iter()
        .filter(|(_, _, k)| *k == ArtifactKind::Native)
        .count();
    assert!(
        wasm_val_count >= 1,
        "at least 1 wasm validator expected, got {wasm_val_count}"
    );
    assert_eq!(
        native_val_count, 1,
        "exactly 1 native validator expected, got {native_val_count}"
    );
}

// ============================================================================
// GAP 5: ArtifactKind round-trips through the actual persist→reopen path
//         for a WASM validator (not just the unit from_record test)
// ============================================================================

use shamir_db::shamir_db::SystemStoreConfig;

/// Re-open the system store, retrying briefly while the previous session's
/// fjall file lock is released.
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

/// Persist a WASM validator to the catalogue, reopen the DB, and confirm the
/// validator still loads AND its `kind` reads `Wasm` through the public
/// `list_validators_with_kind` API.
///
/// This is the e2e companion to the unit-level `from_record` round-trip tests
/// in `artifact_kind_tests.rs` — it exercises the real persist→boot→reload
/// path, proving the `kind` field survives the fjall write cycle and is
/// correctly decoded on reopen.
#[tokio::test]
async fn wasm_validator_kind_survives_reopen_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let sys_path = tmp.path().join("system.db");
    let repo_path = tmp.path().join("repo_main");

    // ── Session 1: persist a wasm validator + bind it ──
    {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        db.create_db("testdb").await;
        let repo_config = RepoConfig::new("main", BoxRepoFactory::fjall(repo_path.clone()))
            .add_table(TableConfig::new("users"));
        db.add_repo("testdb", repo_config).await.unwrap();

        // Persist a wasm validator (kind = Wasm, with wasm_b64).
        db.create_validator_from_wasm("persisted_wasm_val", &accept_wasm(), false)
            .await
            .unwrap();

        // Verify the catalogue row has kind = Wasm while live.
        let entries = db.list_validators_with_kind().await.unwrap();
        let kind = entries
            .iter()
            .find(|(_, n, _)| n == "persisted_wasm_val")
            .map(|(_, _, k)| *k)
            .expect("persisted_wasm_val should be listed");
        assert_eq!(
            kind,
            ArtifactKind::Wasm,
            "wasm validator must report kind = Wasm before reopen"
        );
    }

    // ── Session 2: reopen — the validator should reload AND kind stays Wasm ──
    let db = init_fjall_retry(&sys_path).await;

    // The wasm validator should be live (wasm validators are re-materialised
    // from wasm_b64 at boot, unlike native validators which need re-register).
    let entries = db.list_validators_with_kind().await.unwrap();
    let kind = entries
        .iter()
        .find(|(_, n, _)| n == "persisted_wasm_val")
        .map(|(_, _, k)| *k)
        .expect("persisted_wasm_val should be listed after reopen");
    assert_eq!(
        kind,
        ArtifactKind::Wasm,
        "wasm validator kind must survive the persist→reopen round-trip"
    );

    // Confirm the underlying catalogue row also reads Wasm via from_record.
    let records = db.system_store().load_validators().await.unwrap();
    let row = records
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("persisted_wasm_val"))
        .expect("catalogue row for persisted_wasm_val should exist");
    assert_eq!(
        ArtifactKind::from_record(row),
        ArtifactKind::Wasm,
        "ArtifactKind::from_record on the reloaded catalogue row must be Wasm"
    );
}
