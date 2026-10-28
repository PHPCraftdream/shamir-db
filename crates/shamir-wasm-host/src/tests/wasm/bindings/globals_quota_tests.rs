use crate::{
    FnBatch, FnCtx, FunctionError, GlobalVars, GlobalsQuota, ShamirFunction, WasmEngine,
    WasmFunction, WasmLimits,
};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

const WAT: &str = r#"
(module
  (import "shamir_host" "global_set" (func $global_set (param i32 i32 i32 i32)))
  (memory (export "memory") 2)
  (data (i32.const 1024) "a")
  (data (i32.const 1032) "b")
  (data (i32.const 1040) "\01")
  (data (i32.const 1048) "\02")
  (data (i32.const 1056) "\c0")
  (global $bump (mut i32) (i32.const 2048))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param i32 i32) (result i64)
    (call $global_set (i32.const 1024) (i32.const 1) (i32.const 1040) (i32.const 1))
    (call $global_set (i32.const 1032) (i32.const 1) (i32.const 1048) (i32.const 1))
    (i64.or (i64.shl (i64.const 1056) (i64.const 32)) (i64.const 1)))
)
"#;

async fn invoke(
    wat: &str,
    quota: GlobalsQuota,
) -> (Result<QueryValue, FunctionError>, Arc<GlobalVars>) {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let function = WasmFunction::from_wat(engine, wat, WasmLimits::default()).unwrap();
    let globals = Arc::new(GlobalVars::with_quota(quota));
    let result = function
        .call(
            &FnCtx::with_globals(globals.clone()),
            &FnBatch::new(),
            &crate::Params::new(),
        )
        .await;
    (result, globals)
}
fn default_quota() -> GlobalsQuota {
    GlobalsQuota::default()
}

#[tokio::test]
async fn rv_wasm_host_51b448df_guest_global_set_within_quota_succeeds() {
    let (result, globals) = invoke(WAT, default_quota()).await;
    assert!(result.is_ok());
    assert_eq!(globals.get("a"), Some(QueryValue::Int(1)));
    assert_eq!(globals.get("b"), Some(QueryValue::Int(2)));
}
#[tokio::test]
async fn rv_wasm_host_51b448df_guest_global_set_quota_traps() {
    let quota = GlobalsQuota {
        max_entries: 1,
        ..default_quota()
    };
    let (result, globals) = invoke(WAT, quota).await;
    match result {
        Err(FunctionError::Compute(msg)) => assert!(msg.contains("quota")),
        other => panic!("unexpected result: {other:?}"),
    }
    assert_eq!(globals.get("a"), Some(QueryValue::Int(1)));
    assert_eq!(globals.get("b"), None);
}
#[tokio::test]
async fn rv_wasm_host_51b448df_guest_oversize_value_traps_before_read() {
    let wat = r#"(module
      (import "shamir_host" "global_set" (func $global_set (param i32 i32 i32 i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "k")
      (func (export "shamir_alloc") (param i32) (result i32) (i32.const 1024))
      (func (export "shamir_call") (param i32 i32) (result i64)
        (call $global_set (i32.const 0) (i32.const 1) (i32.const 65536) (i32.const 16))
        (i64.const 0)))"#;
    let quota = GlobalsQuota {
        max_value_bytes: 8,
        ..default_quota()
    };
    let (result, globals) = invoke(wat, quota).await;
    match result {
        Err(FunctionError::Compute(msg)) => assert!(msg.contains("quota")),
        other => panic!("unexpected result: {other:?}"),
    }
    assert_eq!(globals.get("k"), None);
}
