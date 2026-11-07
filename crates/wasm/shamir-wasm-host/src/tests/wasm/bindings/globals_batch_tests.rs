use crate::{FnBatch, FnCtx, FunctionError, ShamirFunction, WasmEngine, WasmFunction, WasmLimits};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

const GLOBAL_GET_WAT: &str = r#"
(module
  (import "shamir_host" "global_get" (func $global_get (param i32 i32) (result i64)))
  (memory (export "memory") 2)
  (data (i32.const 1024) "k")
  (global $bump (mut i32) (i32.const 2048))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param i32 i32) (result i64)
    (call $global_get (i32.const 1024) (i32.const 1)))
)
"#;

const BATCH_GET_WAT: &str = r#"
(module
  (import "shamir_host" "batch_get" (func $batch_get (param i32 i32) (result i64)))
  (memory (export "memory") 2)
  (data (i32.const 1024) "k")
  (global $bump (mut i32) (i32.const 2048))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param i32 i32) (result i64)
    (call $batch_get (i32.const 1024) (i32.const 1)))
)
"#;

const MIXED_GLOBAL_GET_WAT: &str = r#"
(module
  (import "shamir_host" "global_get" (func $global_get (param i32 i32) (result i64)))
  (import "shamir_host" "call" (func $call (param i32 i32 i32 i32) (result i64)))
  (memory (export "memory") 2)
  (data (i32.const 1024) "k")
  (global $bump (mut i32) (i32.const 2048))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param i32 i32) (result i64)
    (call $global_get (i32.const 1024) (i32.const 1)))
)
"#;

async fn invoke_global(wat: &str) -> Result<QueryValue, FunctionError> {
    let engine = Arc::new(WasmEngine::new()?);
    let function = WasmFunction::from_wat(engine, wat, WasmLimits::default())?;
    let globals = Arc::new(crate::runtime::GlobalVars::new());
    globals.set("k", QueryValue::Int(7));
    function
        .call(
            &FnCtx::with_globals(globals),
            &FnBatch::new(),
            &crate::Params::new(),
        )
        .await
}

#[tokio::test]
async fn rv_wasm_host_bd9bc412_present_global_get_returns_value() {
    assert_eq!(
        invoke_global(GLOBAL_GET_WAT).await.unwrap(),
        QueryValue::Int(7)
    );
}

#[tokio::test]
async fn rv_wasm_host_bd9bc412_present_batch_get_returns_value() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let function = WasmFunction::from_wat(engine, BATCH_GET_WAT, WasmLimits::default()).unwrap();
    let batch = FnBatch::new();
    batch.context().put("k", QueryValue::Int(7));
    let result = function
        .call(&FnCtx::new(), &batch, &crate::Params::new())
        .await;
    assert_eq!(result.unwrap(), QueryValue::Int(7));
}

#[tokio::test]
async fn rv_wasm_host_bd9bc412_mixed_async_import_returns_global() {
    assert_eq!(
        invoke_global(MIXED_GLOBAL_GET_WAT).await.unwrap(),
        QueryValue::Int(7)
    );
}
