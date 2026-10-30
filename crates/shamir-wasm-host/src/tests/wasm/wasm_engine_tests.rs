//! Engine lifetime: the epoch ticker must not keep a dropped engine alive.

use crate::{
    FnBatch, FnCtx, Params, ShamirFunction, WasmEngine, WasmEngineOptions, WasmFunction, WasmLimits,
};
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const GROW_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "shamir_alloc") (param i32) (result i32) (i32.const 0))
  (func (export "shamir_call") (param $p i32) (param $n i32) (result i64)
    (if (i32.eq (memory.grow (i32.const 1024)) (i32.const -1)) (then unreachable))
    (i64.or (i64.shl (i64.extend_i32_u (local.get $p)) (i64.const 32)) (i64.extend_i32_u (local.get $n)))))
"#;

#[test]
fn pooling_ceiling_rejects_both_constructors_before_compilation() {
    let engine = Arc::new(
        WasmEngine::with_options(WasmEngineOptions {
            pooling: true,
            pooling_memory_ceiling: 64 * 1024 * 1024,
        })
        .unwrap(),
    );
    assert_eq!(
        engine.pooling_memory_ceiling(),
        Some(64 * 1024 * 1024),
        "pooling unavailable; rejection coverage cannot run"
    );
    let limits = WasmLimits {
        max_memory_bytes: 65 * 1024 * 1024,
        ..Default::default()
    };
    let requested = limits.max_memory_bytes.to_string();
    let available = (64 * 1024 * 1024).to_string();
    let wat_error = WasmFunction::from_wat(engine.clone(), "not wat", limits.clone())
        .err()
        .unwrap()
        .to_string();
    let binary_error = WasmFunction::from_binary(engine, b"invalid", limits)
        .err()
        .unwrap()
        .to_string();
    for error in [wat_error, binary_error] {
        assert!(
            error.contains(&requested)
                && error.contains(&available)
                && error.contains("pooling_memory_ceiling"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn larger_pool_and_on_demand_allow_growth_above_default() {
    for pooling in [true, false] {
        let engine = Arc::new(
            WasmEngine::with_options(WasmEngineOptions {
                pooling,
                pooling_memory_ceiling: 80 * 1024 * 1024,
            })
            .unwrap(),
        );
        if pooling {
            assert_eq!(engine.pooling_memory_ceiling(), Some(80 * 1024 * 1024));
        } else {
            assert_eq!(engine.pooling_memory_ceiling(), None);
        }
        let limits = WasmLimits {
            max_memory_bytes: 72 * 1024 * 1024,
            ..Default::default()
        };
        let function = WasmFunction::from_wat(engine, GROW_WAT, limits).unwrap();
        assert_eq!(
            function
                .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
                .await
                .unwrap(),
            QueryValue::Map(Default::default())
        );
    }
}

#[test]
fn failed_pool_initialization_falls_back_for_both_constructors() {
    let engine = Arc::new(
        WasmEngine::build_with_failing_pool(WasmEngineOptions {
            pooling: true,
            pooling_memory_ceiling: 1,
        })
        .unwrap(),
    );
    assert_eq!(engine.pooling_memory_ceiling(), None);
    let limits = WasmLimits::default();
    assert!(WasmFunction::from_wat(engine.clone(), GROW_WAT, limits.clone()).is_ok());
    assert!(WasmFunction::from_binary(engine, &wat::parse_str(GROW_WAT).unwrap(), limits).is_ok());
}

#[test]
fn on_demand_ignores_pool_ceiling_for_both_constructors() {
    let engine = Arc::new(
        WasmEngine::with_options(WasmEngineOptions {
            pooling: false,
            pooling_memory_ceiling: 1,
        })
        .unwrap(),
    );
    let limits = WasmLimits::default();
    assert!(WasmFunction::from_wat(engine.clone(), GROW_WAT, limits.clone()).is_ok());
    assert!(WasmFunction::from_binary(engine, &wat::parse_str(GROW_WAT).unwrap(), limits).is_ok());
}

#[test]
fn rv_wasm_host_fa126f4a_epoch_ticker_spawn_failure_is_returned() {
    let called = AtomicBool::new(false);
    let result = WasmEngine::build_with_ticker(None, |_| {
        called.store(true, Ordering::SeqCst);
        Err(crate::FunctionError::Compute(
            "injected epoch ticker spawn failure".into(),
        ))
    });

    assert!(
        called.load(Ordering::SeqCst),
        "ticker startup was not attempted"
    );
    match result {
        Err(crate::FunctionError::Compute(message)) => {
            assert!(message.contains("injected epoch ticker spawn failure"));
        }
        Err(other) => panic!("unexpected error: {other:?}"),
        Ok(_) => panic!("expected engine construction failure"),
    }
}

#[test]
fn epoch_ticker_does_not_keep_dropped_engine_alive() {
    let engine = WasmEngine::new().unwrap();
    let weak = engine.engine().weak();
    assert!(weak.upgrade().is_some(), "engine alive while owned");
    drop(engine);
    // A ticker holding a strong clone would keep the inner engine alive forever.
    assert!(
        weak.upgrade().is_none(),
        "epoch ticker thread retains the dropped engine"
    );
}
