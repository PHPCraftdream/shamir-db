use crate::function::{
    FnBatch, FnCtx, FunctionError, FunctionRegistry, Params, ShamirFunction, WasmEngine,
    WasmFunction, WasmLimits,
};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

/// WAT module implementing the identity echo ABI.
///
/// Exports:
/// - `memory` (2 pages = 128 KiB)
/// - `shamir_alloc(len) -> ptr`  — bump allocator starting at offset 1024
/// - `shamir_call(ptr, len) -> i64` — echoes back the same `[ptr, len)` region
const IDENTITY_WAT: &str = r#"
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

/// WAT module that loops forever — used to test fuel exhaustion.
const INFINITE_LOOP_WAT: &str = r#"
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
    (loop (br 0))
    (unreachable)
  )
)
"#;

/// WAT module whose `shamir_call` grows memory by one page each call.
const MEMORY_GROW_WAT: &str = r#"
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
    ;; Grow memory by four pages (256 KiB); trap if grow fails.
    (if (i32.eq (memory.grow (i32.const 4)) (i32.const -1))
      (then (unreachable)))
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

fn build_identity() -> Arc<WasmFunction> {
    let engine = Arc::new(WasmEngine::new().unwrap());
    Arc::new(WasmFunction::from_wat(engine, IDENTITY_WAT, WasmLimits::default()).unwrap())
}

fn build_params() -> Params {
    let mut p = Params::new();
    p.set("x", QueryValue::Int(42));
    p.set("name", QueryValue::Str("hello".into()));
    p
}

#[tokio::test]
async fn wasm_identity_roundtrips_params() {
    let wf = build_identity();
    let params = build_params();
    let expected = QueryValue::Map(params.raw().clone());

    let result = wf
        .call(&FnCtx::new(), &FnBatch::new(), &params)
        .await
        .unwrap();

    assert_eq!(result, expected);
}

#[tokio::test]
async fn wasm_identity_via_registry() {
    let reg = FunctionRegistry::new();
    let wf = build_identity();
    reg.register("echo", wf).unwrap();

    let mut params = Params::new();
    params.set("val", QueryValue::Bool(true));
    let expected = QueryValue::Map(params.raw().clone());

    let result = reg
        .invoke("echo", &FnCtx::new(), &FnBatch::new(), &params)
        .await
        .unwrap();

    assert_eq!(result, expected);
}

#[tokio::test]
async fn wasm_fuel_exhaustion_traps() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let limits = WasmLimits {
        fuel: 1000,
        ..WasmLimits::default()
    };
    let wf = Arc::new(WasmFunction::from_wat(engine, INFINITE_LOOP_WAT, limits).unwrap());

    let result = wf
        .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
        .await;

    let err = result.unwrap_err();
    assert!(
        matches!(err, FunctionError::Compute(_)),
        "expected Compute error, got: {err:?}"
    );
}

#[tokio::test]
async fn wasm_memory_limit_enforced() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    // Each call creates a fresh store. The module starts at 2 pages; the WAT
    // tries to grow by 4 pages (256 KiB) in one shot. With a limit of 3 pages
    // total (192 KiB) the grow should fail and the guest traps.
    let limits = WasmLimits {
        fuel: 1_000_000_000,
        // Allow only 3 pages (192 KiB) total — less than the 2+4=6 the guest wants.
        max_memory_bytes: 3 * 64 * 1024,
    };
    let wf = Arc::new(WasmFunction::from_wat(engine, MEMORY_GROW_WAT, limits).unwrap());

    let mut params = Params::new();
    params.set("k", QueryValue::Int(1));
    let result = wf.call(&FnCtx::new(), &FnBatch::new(), &params).await;
    assert!(result.is_err(), "grow beyond limit should trap");
}
