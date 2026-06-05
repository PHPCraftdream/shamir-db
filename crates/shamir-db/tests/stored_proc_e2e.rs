//! End-to-end tests for **stored procedures / callable getter-functions**
//! (Phase 1 — `BatchOp::Call` + `FunctionInvoker` + `QueryResult.value`).
//!
//! Proves:
//! - A `{ "call": "fn_name", "params": [...] }` batch entry invokes a
//!   registered function through the batch surface.
//! - The function's `QueryValue` return maps to `QueryResult.value` as
//!   `serde_json::Value` (object / array / scalar / null — all four forms).
//! - setuid getter-only: a user without table Read invokes a setuid
//!   procedure and receives data; without setuid the call is denied.

use std::sync::Arc;

use async_trait::async_trait;

use shamir_db::ShamirDb;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, ShamirFunction};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::batch::BatchOp;
use shamir_query_types::call::CallOp;
use shamir_query_types::filter::FilterValue;
use shamir_types::access::{Actor, Mode, ResourceMeta, ResourcePath};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

// ============================================================================
// Native test procedures
// ============================================================================

/// Returns an object: `{ "sum": a + b }` where a = params["0"], b = params["1"].
struct AddProc;

#[async_trait]
impl ShamirFunction for AddProc {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let a = match params.get("0") {
            Ok(QueryValue::Int(v)) => *v,
            _ => 0,
        };
        let b = match params.get("1") {
            Ok(QueryValue::Int(v)) => *v,
            _ => 0,
        };
        let mut map = new_map();
        map.insert("sum".to_string(), QueryValue::Int(a + b));
        Ok(QueryValue::Map(map))
    }
}

/// Returns an array: the params "args" echoed back.
struct EchoArrayProc;

#[async_trait]
impl ShamirFunction for EchoArrayProc {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        match params.get("args") {
            Ok(qv) => Ok(qv.clone()),
            Err(_) => Ok(QueryValue::List(vec![])),
        }
    }
}

/// Returns a scalar integer: 42.
struct ScalarProc;

#[async_trait]
impl ShamirFunction for ScalarProc {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        Ok(QueryValue::Int(42))
    }
}

/// Returns null.
struct NullProc;

#[async_trait]
impl ShamirFunction for NullProc {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        Ok(QueryValue::Null)
    }
}

/// Reads the `secrets` table through the DB gateway and returns the row count.
struct TableReader;

#[async_trait]
impl ShamirFunction for TableReader {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let gw = ctx
            .db_gateway()
            .ok_or_else(|| FunctionError::Compute("no db gateway".to_string()))?;
        let rows = gw
            .query("main", "secrets", None)
            .await
            .map_err(FunctionError::Compute)?;
        Ok(QueryValue::Int(rows.len() as i64))
    }
}

// ============================================================================
// Helpers
// ============================================================================

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    shamir
}

/// Register a native function with a catalogue entry (for setuid/mode).
async fn register_native_fn(shamir: &ShamirDb, name: &str, f: Arc<dyn ShamirFunction>) {
    let empty_wasm = wat::parse_str("(module)").unwrap();
    shamir
        .create_function_from_wasm_as(name, &empty_wasm, false, Actor::System)
        .await
        .unwrap();
    shamir.functions().replace(name, f);
}

/// Register a native function with owner + mode (for setuid tests).
async fn register_native_fn_owned(
    shamir: &ShamirDb,
    name: &str,
    f: Arc<dyn ShamirFunction>,
    owner: Actor,
    mode: u16,
) {
    let empty_wasm = wat::parse_str("(module)").unwrap();
    shamir
        .create_function_from_wasm_as(name, &empty_wasm, false, owner.clone())
        .await
        .unwrap();
    shamir.functions().replace(name, f);
    shamir
        .set_resource_meta(
            &ResourcePath::function(name),
            &ResourceMeta {
                owner,
                group: None,
                mode,
            },
        )
        .await
        .unwrap();
}

// ============================================================================
// Tests: four result forms (object, array, scalar, null)
// ============================================================================

#[tokio::test]
async fn call_returns_object() {
    let shamir = setup_shamir().await;
    register_native_fn(&shamir, "add", Arc::new(AddProc)).await;

    let mut b = Batch::new();
    b.id("call_obj");
    b.op(
        "result",
        BatchOp::Call(CallOp {
            call: "add".into(),
            params: vec![FilterValue::Int(10), FilterValue::Int(32)],
            repo: "main".into(),
        }),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let qr = &resp.results["result"];
    assert!(
        qr.records.is_empty(),
        "records should be empty for a call result"
    );
    let value = qr.value.as_ref().expect("value must be Some for a call");
    assert_eq!(
        value,
        &serde_json::json!({
            "sum": 42
        }),
        "object return: sum of 10 + 32"
    );
}

#[tokio::test]
async fn call_returns_array() {
    let shamir = setup_shamir().await;
    register_native_fn(&shamir, "echo", Arc::new(EchoArrayProc)).await;

    let mut b = Batch::new();
    b.id("call_arr");
    b.op(
        "result",
        BatchOp::Call(CallOp {
            call: "echo".into(),
            params: vec![
                FilterValue::Int(1),
                FilterValue::String("hello".into()),
                FilterValue::Bool(true),
            ],
            repo: "main".into(),
        }),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let qr = &resp.results["result"];
    let value = qr.value.as_ref().expect("value must be Some");
    assert_eq!(
        value,
        &serde_json::json!([1, "hello", true]),
        "array return: echo of params"
    );
}

#[tokio::test]
async fn call_returns_scalar() {
    let shamir = setup_shamir().await;
    register_native_fn(&shamir, "forty_two", Arc::new(ScalarProc)).await;

    let mut b = Batch::new();
    b.id("call_scalar");
    b.op(
        "result",
        BatchOp::Call(CallOp {
            call: "forty_two".into(),
            params: vec![],
            repo: "main".into(),
        }),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let qr = &resp.results["result"];
    let value = qr.value.as_ref().expect("value must be Some");
    assert_eq!(value, &serde_json::json!(42), "scalar return: 42");
}

#[tokio::test]
async fn call_returns_null() {
    let shamir = setup_shamir().await;
    register_native_fn(&shamir, "null_fn", Arc::new(NullProc)).await;

    let mut b = Batch::new();
    b.id("call_null");
    b.op(
        "result",
        BatchOp::Call(CallOp {
            call: "null_fn".into(),
            params: vec![],
            repo: "main".into(),
        }),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let qr = &resp.results["result"];
    let value = qr
        .value
        .as_ref()
        .expect("value must be Some (even for null)");
    assert_eq!(value, &serde_json::Value::Null, "null return");
}

// ============================================================================
// Tests: setuid getter-only data firewall
// ============================================================================

/// Setup: db "testdb", repo "main", table "secrets" with two rows, locked
/// to owner User(A) with mode 0o750.
async fn setup_with_secrets() -> ShamirDb {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id("setup");
    b.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["secrets"]),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut seed = Batch::new();
    seed.id("seed");
    seed.insert(
        "ins",
        insert("secrets").rows([
            doc! { "id" => 1, "label" => "alpha" },
            doc! { "id" => 2, "label" => "beta" },
        ]),
    );
    shamir
        .execute("testdb", &seed.to_request_via_msgpack())
        .await
        .unwrap();

    shamir
}

async fn lock_table_to(shamir: &ShamirDb, owner: Actor) {
    shamir
        .set_resource_meta(
            &ResourcePath::table("testdb", "main", "secrets"),
            &ResourceMeta {
                owner,
                group: None,
                mode: 0o750,
            },
        )
        .await
        .unwrap();
}

/// User(B) calls a setuid function owned by User(A) via the batch surface
/// `{ "call": "read_secrets" }` — succeeds, receives the row count.
#[tokio::test]
async fn setuid_call_lets_stranger_read_via_owner() {
    let user_a = Actor::User(1001);
    let user_b = Actor::User(2002);

    let shamir = setup_with_secrets().await;
    lock_table_to(&shamir, user_a.clone()).await;

    // setuid reader owned by A; mode 0o4751 = setuid + owner rwx + group r-x +
    // other --x (B may invoke).
    register_native_fn_owned(
        &shamir,
        "read_secrets",
        Arc::new(TableReader),
        user_a.clone(),
        Mode::with_setuid(0o4751, true),
    )
    .await;

    // B invokes via batch Call.
    let mut b = Batch::new();
    b.id("setuid_call");
    b.op(
        "result",
        BatchOp::Call(CallOp {
            call: "read_secrets".into(),
            params: vec![],
            repo: "main".into(),
        }),
    );
    let resp = shamir
        .execute_as(user_b.clone(), "testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let qr = &resp.results["result"];
    let value = qr.value.as_ref().expect("value must be Some");
    assert_eq!(
        value,
        &serde_json::json!(2),
        "setuid getter should see 2 rows via owner A's authority"
    );
}

/// Without setuid, the same call by User(B) is denied.
#[tokio::test]
async fn without_setuid_call_is_denied() {
    let user_a = Actor::User(1001);
    let user_b = Actor::User(2002);

    let shamir = setup_with_secrets().await;
    lock_table_to(&shamir, user_a.clone()).await;

    // NON-setuid reader, other-execute granted.
    // 0o0751 = owner rwx + group r-x + other --x, setuid OFF.
    register_native_fn_owned(
        &shamir,
        "read_secrets",
        Arc::new(TableReader),
        user_a.clone(),
        0o0751,
    )
    .await;

    let mut b = Batch::new();
    b.id("no_setuid_call");
    b.op(
        "result",
        BatchOp::Call(CallOp {
            call: "read_secrets".into(),
            params: vec![],
            repo: "main".into(),
        }),
    );
    let resp = shamir
        .execute_as(user_b.clone(), "testdb", &b.to_request_via_msgpack())
        .await;

    // The batch itself may succeed but the call entry should error.
    // Depending on how the error propagates, it may be a BatchError or the
    // result entry may contain the error.
    match resp {
        Err(e) => {
            let msg = format!("{e}");
            assert!(
                msg.contains("denied") || msg.contains("Denied"),
                "denial must come from the per-table ACL gate, got: {msg}"
            );
        }
        Ok(r) => {
            // If the batch didn't fail outright, it should be missing "result"
            // or the result should indicate failure. This is a successful batch
            // with a failed call — the error propagates as BatchError in the
            // executor's plan_result path.
            panic!(
                "expected a denial error, but batch succeeded with results: {:?}",
                r.results
            );
        }
    }
}
