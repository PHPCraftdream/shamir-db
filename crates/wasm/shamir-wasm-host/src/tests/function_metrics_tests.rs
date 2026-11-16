//! Real-call regression coverage for the wasm-host telemetry mirrors.

use crate::wasm::wasm_function::metrics::local_registry_capacity_totals;
use crate::{
    admission_rejection_totals, call_metrics_snapshot, compile_cache_metrics, verify_wasm_module,
    ADMISSION_REJECTION_REASONS, FnBatch, FnCtx, FuelObserver, FunctionError, FunctionRegistry,
    Params, ShamirFunction, WasmEngine, WasmFunction, WasmLimits,
};
use shamir_metrics::{Histogram, LATENCY_NS_BOUNDS};
use shamir_types::types::value::QueryValue;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

const IDENTITY_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
      (i64.extend_i32_u (local.get $len)))))
"#;

fn metrics_for(db: &str) -> crate::CallMetricsSnapshot {
    call_metrics_snapshot()
        .into_iter()
        .find(|entry| entry.db == db)
        .unwrap_or_else(|| panic!("real call metrics missing for db label {db}"))
}

fn assert_one_root_call(db: &str, fuel: u64, depth: u64) {
    let snapshot = metrics_for(db);
    assert_eq!(snapshot.duration.count, 1, "one depth-0 call is observed");
    assert!(snapshot.duration.sum > 0, "elapsed call duration is recorded");
    assert_eq!(snapshot.duration.bounds, LATENCY_NS_BOUNDS);
    assert_eq!(snapshot.fuel_total, fuel, "fuel comes from real Store consumption");
    assert_eq!(snapshot.depth_high_water, depth);
}

#[test]
fn latency_histogram_places_1500000_ns_in_bucket_four() {
    let histogram = Histogram::new(LATENCY_NS_BOUNDS);
    histogram.observe(1_500_000);
    let snapshot = histogram.snapshot();
    assert!(!snapshot.torn);
    assert_eq!(snapshot.buckets[1], 0);
    assert_eq!(snapshot.buckets[4], 1);
    assert_eq!(snapshot.count, 1);
    assert_eq!(snapshot.sum, 1_500_000);
}

#[test]
fn registry_caps_named_entries_and_aggregates_overflow_in_isolation() {
    let (named, overflow_fuel) = local_registry_capacity_totals();
    assert_eq!(named, 64);
    assert_eq!(overflow_fuel, 6);
}

#[test]
fn rejections_classify_by_reason() {
    assert_eq!(
        ADMISSION_REJECTION_REASONS,
        ["parse", "validation", "import", "operator", "abi", "interface", "limits"]
    );
    let parse_before = admission_rejection_totals()[0].1;
    let result = verify_wasm_module(b"definitely not wasm");
    assert!(result.is_err(), "garbage bytes must be refused");
    let after = admission_rejection_totals();
    assert_eq!(after[0].0, "parse");
    assert!(after[0].1 > parse_before, "parse rejection counter must increase");
}

#[test]
fn cache_misses_track_uncached_compiles_without_global_exact_delta() {
    let engine = WasmEngine::new().unwrap();
    let before = compile_cache_metrics();
    engine.compile_optimized(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]).unwrap();
    let after = compile_cache_metrics();
    assert!(after.misses >= before.misses + 1);
}

#[tokio::test]
async fn wasm_call_success_records_duration_fuel_and_depth_from_store() {
    let db = "metrics_real_success_call";
    let observer = FuelObserver::new();
    let function = WasmFunction::from_wat(
        Arc::new(WasmEngine::new().unwrap()),
        IDENTITY_WAT,
        WasmLimits::default(),
    )
    .unwrap();
    let mut params = Params::new();
    params.set("value", QueryValue::Int(19));
    let expected = QueryValue::Map(params.raw().clone());
    let ctx = FnCtx::new()
        .with_db_name(db)
        .with_fuel_observer(observer.clone());

    assert_eq!(
        function.call(&ctx, &FnBatch::new(), &params).await.unwrap(),
        expected
    );
    let burned = observer.burned();
    assert!(burned > 0, "the observer independently sees actual fuel use");
    assert_one_root_call(db, burned, 0);
}

#[tokio::test]
async fn wasm_call_trap_records_duration_and_consumed_fuel() {
    let db = "metrics_real_trap_call";
    let observer = FuelObserver::new();
    let wat = r#"
(module
  (memory (export "memory") 1)
  (func (export "shamir_alloc") (param i32) (result i32) (i32.const 1024))
  (func (export "shamir_call") (param i32 i32) (result i64) (unreachable)))
"#;
    let function = WasmFunction::from_wat(
        Arc::new(WasmEngine::new().unwrap()),
        wat,
        WasmLimits::default(),
    )
    .unwrap();
    let ctx = FnCtx::new()
        .with_db_name(db)
        .with_fuel_observer(observer.clone());

    assert!(matches!(
        function.call(&ctx, &FnBatch::new(), &Params::new()).await,
        Err(FunctionError::Compute(_))
    ));
    let burned = observer.burned();
    assert!(burned > 0);
    assert_one_root_call(db, burned, 0);
}

#[tokio::test]
async fn nested_wasm_calls_count_duration_once_and_raise_depth_high_water() {
    let db = "metrics_real_nested_call";
    let observer = FuelObserver::new();
    let parent_wat = r#"
(module
  (import "shamir_host" "call" (func $host_call (param i32 i32 i32 i32) (result i64)))
  (memory (export "memory") 2)
  (data (i32.const 0) "child")
  (data (i32.const 16) "\80")
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (drop (call $host_call (i32.const 0) (i32.const 5) (i32.const 16) (i32.const 1)))
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
      (i64.extend_i32_u (local.get $len)))))
"#;
    let engine = Arc::new(WasmEngine::new().unwrap());
    let parent = Arc::new(WasmFunction::from_wat(engine.clone(), parent_wat, WasmLimits::default()).unwrap());
    let child = Arc::new(WasmFunction::from_wat(engine, IDENTITY_WAT, WasmLimits::default()).unwrap());
    let registry = Arc::new(FunctionRegistry::new());
    registry.register("parent", parent).unwrap();
    registry.register("child", child).unwrap();
    let ctx = FnCtx::new()
        .with_registry(registry.clone())
        .with_depth_limit(4)
        .with_db_name(db)
        .with_fuel_observer(observer.clone());

    registry
        .invoke("parent", &ctx, &FnBatch::new(), &Params::new())
        .await
        .unwrap();
    let burned = observer.burned();
    assert!(burned > 0);
    assert_one_root_call(db, burned, 1);
}

struct PendingDb {
    entered: AtomicBool,
}

#[async_trait::async_trait]
impl crate::DbGateway for PendingDb {
    async fn get(
        &self,
        _repo: &str,
        _table: &str,
        _key: QueryValue,
    ) -> Result<Option<QueryValue>, crate::DbGatewayError> {
        self.entered.store(true, Ordering::SeqCst);
        std::future::pending().await
    }

    async fn insert(
        &self,
        _repo: &str,
        _table: &str,
        _doc: QueryValue,
    ) -> Result<QueryValue, crate::DbGatewayError> {
        Err(crate::DbGatewayError::Execution("unused".into()))
    }

    async fn query(
        &self,
        _repo: &str,
        _table: &str,
        _filter: Option<QueryValue>,
    ) -> Result<Vec<QueryValue>, crate::DbGatewayError> {
        Err(crate::DbGatewayError::Execution("unused".into()))
    }

    async fn execute(&self, _request: &[u8]) -> Result<Vec<u8>, crate::DbGatewayError> {
        Err(crate::DbGatewayError::Execution("unused".into()))
    }
}

#[tokio::test]
async fn cancelled_wasm_call_records_duration_and_settled_fuel() {
    let db = "metrics_real_cancelled_call";
    let observer = FuelObserver::new();
    let gateway = Arc::new(PendingDb {
        entered: AtomicBool::new(false),
    });
    let wat = r#"
(module
  (import "shamir_host" "db_get" (func $get (param i32 i32 i32 i32) (result i64)))
  (memory (export "memory") 1)
  (data (i32.const 0) "t")
  (data (i32.const 16) "\c0")
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param i32 i32) (result i64)
    (drop (call $get (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 1)))
    (i64.const 0)))
"#;
    let function = WasmFunction::from_wat(
        Arc::new(WasmEngine::new().unwrap()),
        wat,
        WasmLimits::default(),
    )
    .unwrap();
    let ctx = FnCtx::new()
        .with_db_name(db)
        .with_db(gateway.clone(), "metrics_repo".into())
        .with_fuel_observer(observer.clone());
    let batch = FnBatch::new();
    let params = Params::new();
    let mut call = Box::pin(function.call(&ctx, &batch, &params));
    let waker = Waker::noop();
    let mut task_context = Context::from_waker(waker);
    let mut reached_gateway = false;
    for _ in 0..100_000 {
        assert!(
            matches!(call.as_mut().poll(&mut task_context), Poll::Pending),
            "call unexpectedly completed before cancellation"
        );
        if gateway.entered.load(Ordering::SeqCst) && observer.gate_observed() > 0 {
            reached_gateway = true;
            break;
        }
    }
    assert!(reached_gateway, "call never reached the pending db_get gateway");
    drop(call);

    let burned = observer.burned();
    assert!(burned > 0);
    assert_one_root_call(db, burned, 0);
}

#[test]
fn fn_ctx_carries_db_name() {
    let ctx = FnCtx::new().with_db_name("metrics_db");
    assert_eq!(ctx.db_name(), Some("metrics_db"));
    assert_eq!(FnCtx::new().db_name(), None);
}
