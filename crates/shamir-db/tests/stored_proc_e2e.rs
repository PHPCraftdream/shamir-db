//! End-to-end tests for **stored procedures / callable getter-functions**
//! (Phase 1 — `BatchOp::Call` + `FunctionInvoker` + `QueryResult.value`).
//!
//! Proves:
//! - A `{ "call": "fn_name", "params": [...] }` batch entry invokes a
//!   registered function through the batch surface.
//! - The function's `QueryValue` return maps to `QueryResult.value` as
//!   `QueryValue` (object / array / scalar / null — all four forms).
//! - setuid getter-only: a user without table Read invokes a setuid
//!   procedure and receives data; without setuid the call is denied.
//!
//! ## Phase 2 — Call in dependency graph
//!
//! Proves:
//! - A Call with `$query` params is topologically ordered after its
//!   dependency (execution plan shows two stages).
//! - The resolved `$query` value arrives as a non-null param to the
//!   procedure (echo test).
//! - A Call's `QueryResult.value` can itself be referenced by a later
//!   Read via `$query` in a `where` filter (call-result-as-ref).
//! - A Call with a `$query` param referencing an unknown alias fails
//!   at planning time with `UnknownAlias`.

use std::sync::Arc;

use async_trait::async_trait;

use shamir_db::ShamirDb;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, ShamirFunction};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::q;
use shamir_query_builder::val::qref;
use shamir_query_builder::write::insert;
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
    // Exercises the q!(call ...) macro on the e2e path (object return).
    b.op("result", q!(call add(10_i64, 32)));
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
    let mut expected_map = new_map();
    expected_map.insert("sum".to_string(), QueryValue::Int(42));
    assert_eq!(
        value,
        &QueryValue::Map(expected_map),
        "object return: sum of 10 + 32"
    );
}

#[tokio::test]
async fn call_returns_array() {
    let shamir = setup_shamir().await;
    register_native_fn(&shamir, "echo", Arc::new(EchoArrayProc)).await;

    let mut b = Batch::new();
    b.id("call_arr");
    b.call(
        "result",
        "echo",
        vec![
            FilterValue::from(1_i64),
            FilterValue::from("hello"),
            FilterValue::from(true),
        ],
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let qr = &resp.results["result"];
    let value = qr.value.as_ref().expect("value must be Some");
    assert_eq!(
        value,
        &QueryValue::List(vec![
            QueryValue::Int(1),
            QueryValue::Str("hello".to_string()),
            QueryValue::Bool(true),
        ]),
        "array return: echo of params"
    );
}

#[tokio::test]
async fn call_returns_scalar() {
    let shamir = setup_shamir().await;
    register_native_fn(&shamir, "forty_two", Arc::new(ScalarProc)).await;

    let mut b = Batch::new();
    b.id("call_scalar");
    // Exercises the q!(call ...) macro on the e2e path (scalar return).
    b.op("result", q!(call forty_two()));
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let qr = &resp.results["result"];
    let value = qr.value.as_ref().expect("value must be Some");
    assert_eq!(value, &QueryValue::Int(42), "scalar return: 42");
}

#[tokio::test]
async fn call_returns_null() {
    let shamir = setup_shamir().await;
    register_native_fn(&shamir, "null_fn", Arc::new(NullProc)).await;

    let mut b = Batch::new();
    b.id("call_null");
    b.call("result", "null_fn", [] as [FilterValue; 0]);
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let qr = &resp.results["result"];
    let value = qr
        .value
        .as_ref()
        .expect("value must be Some (even for null)");
    assert_eq!(value, &QueryValue::Null, "null return");
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
    b.call("result", "read_secrets", [] as [FilterValue; 0]);
    let resp = shamir
        .execute_as(user_b.clone(), "testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let qr = &resp.results["result"];
    let value = qr.value.as_ref().expect("value must be Some");
    assert_eq!(
        value,
        &QueryValue::Int(2),
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
    b.call("result", "read_secrets", [] as [FilterValue; 0]);
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

// ============================================================================
// Phase 2 — Call in dependency graph: native test procedures
// ============================================================================

/// Echoes its first positional param back as `value`.
/// Guest reads: `params.get("0")`.
struct EchoFirstParamProc;

#[async_trait]
impl ShamirFunction for EchoFirstParamProc {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        match params.get("0") {
            Ok(qv) => Ok(qv.clone()),
            Err(_) => Ok(QueryValue::Null),
        }
    }
}

/// Always returns `{ "id": 5 }`.
struct ConstObjectProc;

#[async_trait]
impl ShamirFunction for ConstObjectProc {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let mut map = new_map();
        map.insert("id".to_string(), QueryValue::Int(5));
        Ok(QueryValue::Map(map))
    }
}

// ============================================================================
// Phase 2 tests
// ============================================================================

/// **params-from-ref**: a Read result feeds into a Call's `$query` param.
///
/// Batch layout:
///   - `q1`: Read from `items` (returns rows with `id` field).
///   - `p`:  Call `echo_first` with params `[{ "$query": "@q1[0].id" }]`.
///
/// Assertions:
///   (a) `execution_plan` has two stages — q1 first, p second.
///   (b) p's `value` equals the `id` from q1's first record (not Null).
#[tokio::test]
async fn phase2_params_from_read_ref() {
    let shamir = setup_shamir().await;

    // Create repo + table + seed data.
    let mut setup = Batch::new();
    setup.id("setup");
    setup.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["items"]),
    );
    shamir
        .execute("testdb", &setup.to_request_via_msgpack())
        .await
        .unwrap();

    let mut seed = Batch::new();
    seed.id("seed");
    seed.insert(
        "ins",
        insert("items").rows([
            doc! { "id" => 7, "name" => "alpha" },
            doc! { "id" => 9, "name" => "beta" },
        ]),
    );
    shamir
        .execute("testdb", &seed.to_request_via_msgpack())
        .await
        .unwrap();

    // Register the echo procedure.
    register_native_fn(&shamir, "echo_first", Arc::new(EchoFirstParamProc)).await;

    // Build the dependency batch: q1 (Read) -> p (Call with $query ref).
    let mut b = Batch::new();
    b.id("phase2_params");
    b.op(
        "q1",
        shamir_query_builder::query::Query::from("items")
            .order_by_asc("id")
            .limit(1),
    );
    b.call("p", "echo_first", [qref("q1", "[0].id")]);
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // (a) execution_plan: q1 in stage 0, p in stage 1.
    assert!(
        resp.execution_plan.len() >= 2,
        "expected at least 2 stages, got: {:?}",
        resp.execution_plan
    );
    assert!(
        resp.execution_plan[0].contains(&"q1".to_string()),
        "stage 0 must contain q1: {:?}",
        resp.execution_plan
    );
    // p must be in a later stage than q1.
    let p_stage = resp
        .execution_plan
        .iter()
        .position(|stage| stage.contains(&"p".to_string()))
        .expect("p must appear in execution_plan");
    assert!(
        p_stage >= 1,
        "p must be in stage >= 1, found in stage {}: {:?}",
        p_stage,
        resp.execution_plan
    );

    // (b) p's value is the resolved id from q1's first record (7).
    let p_result = &resp.results["p"];
    let p_value = p_result
        .value
        .as_ref()
        .expect("call result must have value");
    assert_eq!(
        p_value,
        &QueryValue::Int(7),
        "echo_first should receive the resolved id=7 from q1[0].id"
    );
}

/// **params-from-call-ref**: a Call result feeds into another Call's param.
///
/// Batch layout:
///   - `p1`: Call `const_obj` (returns `{"id": 5}`).
///   - `p2`: Call `echo_first` with params `[{ "$query": "@p1.id" }]`.
///
/// Assertions:
///   (a) Two stages in execution_plan.
///   (b) p2's `value` == 5.
#[tokio::test]
async fn phase2_params_from_call_ref() {
    let shamir = setup_shamir().await;
    register_native_fn(&shamir, "const_obj", Arc::new(ConstObjectProc)).await;
    register_native_fn(&shamir, "echo_first", Arc::new(EchoFirstParamProc)).await;

    let mut b = Batch::new();
    b.id("phase2_call_chain");
    b.call("p1", "const_obj", [] as [FilterValue; 0]);
    b.call("p2", "echo_first", [qref("p1", ".id")]);
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // (a) Two stages.
    assert!(
        resp.execution_plan.len() >= 2,
        "expected at least 2 stages, got: {:?}",
        resp.execution_plan
    );

    // (b) p2 echoed the resolved value.
    let p2_value = resp.results["p2"]
        .value
        .as_ref()
        .expect("p2 must have value");
    assert_eq!(
        p2_value,
        &QueryValue::Int(5),
        "echo_first should receive id=5 from p1.id"
    );
}

/// **call-result-as-ref**: a Call's `value` is used in a Read's `where` filter.
///
/// Batch layout:
///   - `p`: Call `const_obj` (returns `{"id": 5}`).
///   - `q`: Read from `items` where `id == { "$query": "@p.id" }`.
///
/// Assertions:
///   (a) Two stages.
///   (b) q returns exactly the row with id=5.
#[tokio::test]
async fn phase2_call_result_as_read_filter_ref() {
    let shamir = setup_shamir().await;

    // Create repo + table + seed.
    let mut setup = Batch::new();
    setup.id("setup");
    setup.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["items"]),
    );
    shamir
        .execute("testdb", &setup.to_request_via_msgpack())
        .await
        .unwrap();

    let mut seed = Batch::new();
    seed.id("seed");
    seed.insert(
        "ins",
        insert("items").rows([
            doc! { "id" => 3, "name" => "gamma" },
            doc! { "id" => 5, "name" => "delta" },
            doc! { "id" => 8, "name" => "epsilon" },
        ]),
    );
    shamir
        .execute("testdb", &seed.to_request_via_msgpack())
        .await
        .unwrap();

    register_native_fn(&shamir, "const_obj", Arc::new(ConstObjectProc)).await;

    // Build: p (Call) -> q (Read with $query filter).
    let mut b = Batch::new();
    b.id("phase2_call_to_read");
    b.call("p", "const_obj", [] as [FilterValue; 0]);
    b.op(
        "q",
        shamir_query_builder::query::Query::from("items").where_eq("id", qref("p", ".id")),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // (a) Two stages.
    assert!(
        resp.execution_plan.len() >= 2,
        "expected at least 2 stages, got: {:?}",
        resp.execution_plan
    );
    let q_stage = resp
        .execution_plan
        .iter()
        .position(|stage| stage.contains(&"q".to_string()))
        .expect("q must appear in plan");
    assert!(
        q_stage >= 1,
        "q must run after p: {:?}",
        resp.execution_plan
    );

    // (b) q returned exactly the row with id=5.
    let q_result = &resp.results["q"];
    assert_eq!(
        q_result.records.len(),
        1,
        "should match exactly one row (id=5), got: {:?}",
        q_result.records
    );
    assert_eq!(
        q_result.records[0].get_value_i64("id"),
        Some(5),
        "matched row must have id=5"
    );
    assert_eq!(
        q_result.records[0].get_value_str("name"),
        Some("delta"),
        "matched row must be 'delta'"
    );
}

/// **unknown-alias in call params**: referencing a non-existent alias in
/// a Call's `$query` param fails at planning time with `UnknownAlias`.
#[tokio::test]
async fn phase2_unknown_alias_in_call_params() {
    let shamir = setup_shamir().await;
    register_native_fn(&shamir, "echo_first", Arc::new(EchoFirstParamProc)).await;

    let mut b = Batch::new();
    b.id("phase2_unknown");
    b.call("p", "echo_first", [qref("no_such_alias", ".x")]);
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect_err("should fail with UnknownAlias");
    let msg = format!("{err}");
    assert!(
        msg.contains("unknown") || msg.contains("Unknown"),
        "error should mention unknown alias, got: {msg}"
    );
}
