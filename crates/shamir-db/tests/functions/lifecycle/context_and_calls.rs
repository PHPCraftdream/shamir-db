//! Batch context exchange, global variables, host-import bridge,
//! env seeding, and function-calls-function end-to-end tests.

use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::ShamirDb;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, ShamirFunction};
use shamir_types::types::value::QueryValue;

use super::support::{compile_or_skip, DOUBLE_SRC};

// ── Slice 5: native test functions for batch context + globals ────────

/// Writes `batch.put("tmp", Int(99))` and returns `Null`.
struct Producer;

#[async_trait]
impl ShamirFunction for Producer {
    async fn call(
        &self,
        _ctx: &FnCtx,
        batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        batch.put("tmp", QueryValue::Int(99));
        Ok(QueryValue::Null)
    }
}

/// Returns `batch.get("tmp")`, falling back to `Null`.
struct Consumer;

#[async_trait]
impl ShamirFunction for Consumer {
    async fn call(
        &self,
        _ctx: &FnCtx,
        batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        Ok(batch.get("tmp").unwrap_or(QueryValue::Null))
    }
}

/// Reads `ctx.global_get("counter")` (default 0), increments, sets it
/// back, returns the new value.
struct GlobalBump;

#[async_trait]
impl ShamirFunction for GlobalBump {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let current = match ctx.global_get("counter") {
            Some(QueryValue::Int(n)) => n,
            _ => 0,
        };
        let next = current + 1;
        ctx.global_set("counter", QueryValue::Int(next));
        Ok(QueryValue::Int(next))
    }
}

#[tokio::test]
async fn facade_batch_context_exchange() {
    let db = ShamirDb::init_memory().await.unwrap();

    db.functions()
        .register("producer", Arc::new(Producer))
        .unwrap();
    db.functions()
        .register("consumer", Arc::new(Consumer))
        .unwrap();

    let ctx = db.new_batch_context();

    db.invoke_function_with_batch("producer", Params::new(), &ctx)
        .await
        .unwrap();

    let result = db
        .invoke_function_with_batch("consumer", Params::new(), &ctx)
        .await
        .unwrap();

    assert_eq!(result, QueryValue::Int(99));
}

#[tokio::test]
async fn facade_globals_shared() {
    let db = ShamirDb::init_memory().await.unwrap();

    db.functions()
        .register("bump", Arc::new(GlobalBump))
        .unwrap();

    let r1 = db.invoke_function("bump", Params::new()).await.unwrap();
    assert_eq!(r1, QueryValue::Int(1));

    let r2 = db.invoke_function("bump", Params::new()).await.unwrap();
    assert_eq!(r2, QueryValue::Int(2));

    // Verify through the globals accessor directly.
    assert_eq!(db.globals().get("counter"), Some(QueryValue::Int(2)));
}
// ── Slice 6: compiled-SDK host-import bridge tests ───────────────────

/// Source that writes to the batch scratchpad via host import.
const BATCH_PUT_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn put_it(_ctx: Ctx, batch: Batch, params: Params) -> Result<Value> {
    let v: i64 = params.i64("v")?;
    batch.put("tmp", Value::Int(v));
    Ok(Value::Null)
}
"#;

/// Source that reads from the batch scratchpad via host import.
const BATCH_GET_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn get_it(_ctx: Ctx, batch: Batch, _params: Params) -> Result<Value> {
    Ok(batch.get("tmp").unwrap_or(Value::Null))
}
"#;

/// Source that sets a global variable via host import.
const GLOBAL_SET_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn set_g(_ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    _ctx.global_set("g", Value::Int(7));
    Ok(Value::Null)
}
"#;

/// Source that reads a global variable via host import.
const GLOBAL_GET_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn get_g(_ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    Ok(_ctx.global_get("g").unwrap_or(Value::Null))
}
"#;

/// Two separately compiled SDK functions sharing a batch context through
/// the `shamir_host` WASM imports: writer puts Int(99), reader gets it back.
#[tokio::test]
async fn wasm_batch_context_exchange_via_host_imports() {
    let db = ShamirDb::init_memory().await.unwrap();

    if !compile_or_skip(&db, "put_it", BATCH_PUT_SRC).await {
        return;
    }
    if !compile_or_skip(&db, "get_it", BATCH_GET_SRC).await {
        return;
    }

    let batch = db.new_batch_context();

    let mut put_params = Params::new();
    put_params.set("v", QueryValue::Int(99));

    let r = db
        .invoke_function_with_batch("put_it", put_params, &batch)
        .await
        .unwrap();
    assert_eq!(r, QueryValue::Null);

    let result = db
        .invoke_function_with_batch("get_it", Params::new(), &batch)
        .await
        .unwrap();
    assert_eq!(
        result,
        QueryValue::Int(99),
        "reader should see the value the writer put into the shared batch"
    );
}

/// Two separately compiled SDK functions sharing global variables through
/// the `shamir_host` WASM imports: setter writes Int(7), reader gets it back.
#[tokio::test]
async fn wasm_globals_exchange_via_host_imports() {
    let db = ShamirDb::init_memory().await.unwrap();

    if !compile_or_skip(&db, "set_g", GLOBAL_SET_SRC).await {
        return;
    }
    if !compile_or_skip(&db, "get_g", GLOBAL_GET_SRC).await {
        return;
    }

    // set_g runs with the DB's own globals.
    let r = db.invoke_function("set_g", Params::new()).await.unwrap();
    assert_eq!(r, QueryValue::Null);

    // get_g runs in a fresh FnBatch but shares globals through the DB.
    let result = db.invoke_function("get_g", Params::new()).await.unwrap();
    assert_eq!(
        result,
        QueryValue::Int(7),
        "reader should see the global set by the writer"
    );

    // Verify through the DB's globals accessor.
    assert_eq!(db.globals().get("g"), Some(QueryValue::Int(7)));
}

// ── Slice 7: env-seeding facade test ─────────────────────────────────

#[tokio::test]
async fn facade_seeds_shamir_env() {
    std::env::set_var("SHAMIR_S7_FACADE_TEST", "42");
    let db = ShamirDb::init_memory().await.unwrap();
    assert_eq!(
        db.globals().get("env.SHAMIR_S7_FACADE_TEST"),
        Some(QueryValue::Str("42".to_string()))
    );
    std::env::remove_var("SHAMIR_S7_FACADE_TEST");
}
// ── Slice 8a: function-calls-function + async execution ───────────────

/// Source for a function that calls `ctx.call("double", {n: N})` and returns
/// the result.
const CALLER_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn caller(ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n: i64 = params.i64("n")?;
    let args = Value::Map(vec![("n".to_string(), Value::Int(n))]);
    Ok(ctx.call("double", args))
}
"#;

/// Source for a function that unconditionally calls itself (tests depth limit).
const RECURSE_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn recurse(ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    let args = Value::Map(vec![]);
    ctx.call("recurse", args);
    Ok(Value::Null)
}
"#;

/// Source for a caller that puts into the batch, then calls a callee that reads
/// from the same batch key.
const BATCH_WRITER_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn batch_writer(ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    _batch.put("shared_key", Value::Int(123));
    let args = Value::Map(vec![]);
    Ok(ctx.call("batch_reader", args))
}
"#;

const BATCH_READER_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn batch_reader(_ctx: Ctx, batch: Batch, _params: Params) -> Result<Value> {
    Ok(batch.get("shared_key").unwrap_or(Value::Null))
}
"#;

/// A `caller` function invokes `double({n: 21})` and returns the result.
/// Asserts Int(42).
#[tokio::test]
async fn wasm_function_calls_function() {
    let db = ShamirDb::init_memory().await.unwrap();

    if !compile_or_skip(&db, "double", DOUBLE_SRC).await {
        return;
    }
    if !compile_or_skip(&db, "caller", CALLER_SRC).await {
        return;
    }

    let mut params = Params::new();
    params.set("n", QueryValue::Int(21));

    let result = db.invoke_function("caller", params).await.unwrap();
    assert_eq!(
        result,
        QueryValue::Int(42),
        "caller should have received double(21) = 42"
    );
}

/// A function that calls itself unconditionally must hit the depth limit and
/// error (not hang or stack-overflow).
#[tokio::test]
async fn wasm_call_depth_limit() {
    let db = ShamirDb::init_memory().await.unwrap();

    if !compile_or_skip(&db, "recurse", RECURSE_SRC).await {
        return;
    }

    let result = db.invoke_function("recurse", Params::new()).await;
    assert!(
        result.is_err(),
        "recursive call must be rejected by the depth limit"
    );
    let err_msg = format!("{}", result.unwrap_err());
    // The exact wording depends on how the depth-limit trap unwinds through
    // ~32 guest frames. The inner "call depth limit exceeded" message is
    // often swallowed by the wasm backtrace unwinder; what reliably survives
    // is the compute-path marker. The key invariant: it errored (did not
    // hang) and the error came from the WASM compute path.
    assert!(
        err_msg.contains("depth limit")
            || err_msg.contains("call depth")
            || err_msg.contains("shamir_call trap"),
        "error should mention depth limit or call failure, got: {err_msg}"
    );
}

/// A caller puts into the batch, then calls a callee that reads the same key.
/// The callee must see the value the caller wrote — they share the batch context.
#[tokio::test]
async fn wasm_call_shares_batch_context() {
    let db = ShamirDb::init_memory().await.unwrap();

    if !compile_or_skip(&db, "batch_writer", BATCH_WRITER_SRC).await {
        return;
    }
    if !compile_or_skip(&db, "batch_reader", BATCH_READER_SRC).await {
        return;
    }

    let result = db
        .invoke_function("batch_writer", Params::new())
        .await
        .unwrap();
    assert_eq!(
        result,
        QueryValue::Int(123),
        "callee should see the value the caller put into the shared batch"
    );
}
