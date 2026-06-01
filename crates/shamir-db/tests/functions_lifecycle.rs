//! End-to-end lifecycle tests for the function engine (slice 4).
//!
//! Covers: create → invoke → rename → invoke → drop, persistence across
//! re-open, and (toolchain-gated) source compilation.
//!
//! Slice 5 additions: batch context exchange + global variables (native fns).

use async_trait::async_trait;
use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, ShamirFunction};
use shamir_storage::error::DbError;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

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
    wat::parse_str(ECHO_WAT).unwrap()
}

fn make_echo_params() -> Params {
    let mut params = Params::new();
    params.set("x", QueryValue::Int(7));
    params.set("name", QueryValue::Str("hi".to_string()));
    params
}

#[tokio::test]
async fn lifecycle_create_use_rename_use_drop() {
    let db = ShamirDb::init_memory().await.unwrap();

    // 1. Create function from pre-compiled WASM (no toolchain needed).
    db.create_function_from_wasm("echo", &echo_wasm(), false)
        .await
        .unwrap();

    // 2. Invoke it — should echo back the params map.
    let params = make_echo_params();
    let result = db.invoke_function("echo", params.clone()).await.unwrap();
    assert_eq!(result, QueryValue::Map(params.raw().clone()));

    // 3. Rename echo → echo2.
    db.rename_function("echo", "echo2").await.unwrap();

    // Old name is gone.
    assert!(db.invoke_function("echo", params.clone()).await.is_err());

    // New name works.
    let result2 = db.invoke_function("echo2", params.clone()).await.unwrap();
    assert_eq!(result2, QueryValue::Map(params.raw().clone()));

    // 4. List contains echo2, not echo.
    let names = db.list_functions().await.unwrap();
    assert!(names.contains(&"echo2".to_string()));
    assert!(!names.contains(&"echo".to_string()));

    // 5. Drop echo2 → true.
    let dropped = db.drop_function("echo2").await.unwrap();
    assert!(dropped);

    // Invoke should now fail.
    assert!(db.invoke_function("echo2", params.clone()).await.is_err());

    // Drop again → false (already gone).
    let dropped_again = db.drop_function("echo2").await.unwrap();
    assert!(!dropped_again);
}

#[tokio::test]
async fn functions_persist_across_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("system.db");

    // Open, create function, close.
    {
        let db = ShamirDb::init(SystemStoreConfig::Redb(path.clone()))
            .await
            .unwrap();
        db.create_function_from_wasm("echo", &echo_wasm(), false)
            .await
            .unwrap();

        let params = make_echo_params();
        let result = db.invoke_function("echo", params.clone()).await.unwrap();
        assert_eq!(result, QueryValue::Map(params.raw().clone()));
    }

    // Re-open on the same path — function should be reloaded from catalogue.
    {
        let db = ShamirDb::init(SystemStoreConfig::Redb(path.clone()))
            .await
            .unwrap();
        let params = make_echo_params();
        let result = db.invoke_function("echo", params.clone()).await.unwrap();
        assert_eq!(result, QueryValue::Map(params.raw().clone()));
    }
}

const DOUBLE_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn double(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n: i64 = params.i64("n")?;
    Ok(Value::Int(n * 2))
}
"#;

#[tokio::test]
async fn create_from_source_compiles() {
    let db = ShamirDb::init_memory().await.unwrap();

    let res = db
        .create_function_from_source("double", DOUBLE_SRC, false)
        .await;

    match res {
        Ok(()) => {
            let mut params = Params::new();
            params.set("n", QueryValue::Int(21));
            let result = db.invoke_function("double", params).await.unwrap();
            assert_eq!(result, QueryValue::Int(42));
        }
        Err(DbError::Function(msg)) if msg.contains("toolchain unavailable") => {
            eprintln!(
                "SKIP create_from_source_compiles — toolchain unavailable: {}",
                msg
            );
        }
        Err(e) => panic!("unexpected error: {}", e),
    }
}

/// The full requested e2e in ONE flow on a SOURCE-COMPILED function:
/// create → compile → use → rename → use → delete. Toolchain-gated — skips
/// cleanly when cargo / the wasm32 target is absent.
#[tokio::test]
async fn source_function_full_lifecycle() {
    let db = ShamirDb::init_memory().await.unwrap();

    // create + compile (Rust source → wasm).
    match db
        .create_function_from_source("double", DOUBLE_SRC, false)
        .await
    {
        Ok(()) => {}
        Err(DbError::Function(msg)) if msg.contains("toolchain unavailable") => {
            eprintln!("SKIP source_function_full_lifecycle — toolchain unavailable: {msg}");
            return;
        }
        Err(e) => panic!("unexpected error: {e}"),
    }

    let mut params = Params::new();
    params.set("n", QueryValue::Int(21));

    // use.
    let r1 = db.invoke_function("double", params.clone()).await.unwrap();
    assert_eq!(r1, QueryValue::Int(42));

    // rename.
    db.rename_function("double", "times_two").await.unwrap();
    assert!(db.invoke_function("double", params.clone()).await.is_err());

    // use (after rename) — still the compiled artifact, no recompile.
    let r2 = db
        .invoke_function("times_two", params.clone())
        .await
        .unwrap();
    assert_eq!(r2, QueryValue::Int(42));

    // delete.
    assert!(db.drop_function("times_two").await.unwrap());
    assert!(db.invoke_function("times_two", params).await.is_err());
}

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
