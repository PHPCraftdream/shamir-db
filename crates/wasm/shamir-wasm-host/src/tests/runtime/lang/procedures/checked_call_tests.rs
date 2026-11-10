use crate::{
    AdmissionPolicy, FnBatch, FnCtx, FuelObserver, FunctionError, FunctionRegistry, Params,
    RegistryBinding, ResolvedStoredCall, ShamirStage, StoredCallResolver, TierMode, WasmEngine,
    WasmFunction, WasmLimits,
};
use async_trait::async_trait;
use shamir_lang::{api::CompileLimits, CompileOptions};
use shamir_types::types::value::QueryValue;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use tokio::sync::Notify;

struct FailClosed;

#[async_trait]
impl StoredCallResolver for FailClosed {
    async fn resolve(
        &self,
        _name: &str,
        _parent: &FnCtx,
    ) -> Result<ResolvedStoredCall, FunctionError> {
        Err(FunctionError::NotFound("target".into()))
    }
}

struct Const;

#[async_trait]
impl crate::ShamirFunction for Const {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        Ok(QueryValue::Int(7))
    }
}

struct EnteredProbe(Arc<AtomicBool>);

#[async_trait]
impl crate::ShamirFunction for EnteredProbe {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        self.0.store(true, Ordering::SeqCst);
        Ok(QueryValue::Int(7))
    }
}

struct CapturedCheckedResolver {
    binding: RegistryBinding,
}

#[async_trait]
impl StoredCallResolver for CapturedCheckedResolver {
    async fn resolve(
        &self,
        _name: &str,
        _parent: &FnCtx,
    ) -> Result<ResolvedStoredCall, FunctionError> {
        Err(FunctionError::NotFound("target".into()))
    }

    async fn resolve_checked(
        &self,
        name: &str,
        parent: &FnCtx,
        expected_signature: &[u8],
    ) -> Result<ResolvedStoredCall, FunctionError> {
        assert_eq!(name, "target");
        assert_eq!(expected_signature, b"procedure()->i64");
        Ok(ResolvedStoredCall::new(&self.binding, parent.clone(), None))
    }
}

struct CancelFirstLeaf {
    entered: Arc<Notify>,
    first_call: Arc<AtomicBool>,
}

#[async_trait]
impl crate::ShamirFunction for CancelFirstLeaf {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        if self.first_call.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(QueryValue::Int(7))
    }
}

struct CheckedCaller;

#[async_trait]
impl crate::ShamirFunction for CheckedCaller {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        ctx.call_stored_checked("target", b"procedure()->i64", batch, params)
            .await
    }
}

#[tokio::test]
async fn checked_call_without_captured_root_fails_closed() {
    let ctx = FnCtx::new().with_stored_call_resolver(Arc::new(FailClosed));
    let error = ctx
        .call_stored_checked(
            "target",
            b"procedure()->i64",
            &FnBatch::new(),
            &Params::new(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, FunctionError::Compute(message) if message.contains("captured root invocation"))
    );
}

#[tokio::test]
async fn checked_call_without_resolver_fails_closed() {
    let registry = Arc::new(FunctionRegistry::new());
    registry.register("target", Arc::new(Const)).unwrap();
    registry
        .register("caller", Arc::new(CheckedCaller))
        .unwrap();
    let error = registry
        .invoke(
            "caller",
            &FnCtx::new().with_registry(registry.clone()),
            &FnBatch::new(),
            &Params::new(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, FunctionError::Compute(message) if message.contains("catalogue resolver"))
    );
}

#[tokio::test]
async fn default_checked_resolver_fails_closed() {
    let registry = Arc::new(FunctionRegistry::new());
    registry.register("target", Arc::new(Const)).unwrap();
    registry
        .register("caller", Arc::new(CheckedCaller))
        .unwrap();
    let ctx = FnCtx::new()
        .with_registry(registry.clone())
        .with_stored_call_resolver(Arc::new(FailClosed));
    let error = registry
        .invoke("caller", &ctx, &FnBatch::new(), &Params::new())
        .await
        .unwrap_err();
    assert!(
        matches!(error, FunctionError::Compute(message) if message.contains("not supported by this resolver"))
    );
}

#[tokio::test]
async fn cancelled_checked_call_cleans_active_path_and_allows_reentry() {
    let registry = Arc::new(FunctionRegistry::new());
    let entered = Arc::new(Notify::new());
    let first_call = Arc::new(AtomicBool::new(true));
    registry
        .register(
            "target",
            Arc::new(CancelFirstLeaf {
                entered: Arc::clone(&entered),
                first_call,
            }),
        )
        .unwrap();
    let resolver = Arc::new(CapturedCheckedResolver {
        binding: registry.capture("target").expect("captured leaf binding"),
    });
    registry
        .register("caller", Arc::new(CheckedCaller))
        .unwrap();
    let ctx = FnCtx::new()
        .with_registry(Arc::clone(&registry))
        .with_stored_call_resolver(resolver);

    let batch = FnBatch::new();
    let params = Params::new();
    let mut cancelled = Box::pin(registry.invoke("caller", &ctx, &batch, &params));
    tokio::select! {
        _ = entered.notified() => {}
        result = &mut cancelled => panic!("checked call completed before leaf blocked: {result:?}"),
    }
    drop(cancelled);

    let result = registry
        .invoke("caller", &ctx, &batch, &params)
        .await
        .expect("same context can call target after cancellation");
    assert_eq!(result, QueryValue::Int(7));
    assert_eq!(ctx.active_call_path().len(), 0);
}

const COMPILED_TEST_BUDGET: u64 = 1_000_000_000;

struct CheckedCallDb {
    hit: AtomicBool,
}

#[async_trait]
impl crate::DbGateway for CheckedCallDb {
    async fn get(
        &self,
        _repo: &str,
        _table: &str,
        _key: QueryValue,
    ) -> Result<Option<QueryValue>, crate::DbGatewayError> {
        if !self.hit.swap(true, Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        let mut record = shamir_types::types::common::new_map();
        record.insert("id".to_string(), QueryValue::Int(7));
        Ok(Some(QueryValue::Map(record)))
    }

    async fn insert(
        &self,
        _: &str,
        _: &str,
        _: QueryValue,
    ) -> Result<QueryValue, crate::DbGatewayError> {
        Err(crate::DbGatewayError::Execution("unused".into()))
    }

    async fn query(
        &self,
        _: &str,
        _: &str,
        _: Option<QueryValue>,
    ) -> Result<Vec<QueryValue>, crate::DbGatewayError> {
        Err(crate::DbGatewayError::Execution("unused".into()))
    }

    async fn execute(&self, _: &[u8]) -> Result<Vec<u8>, crate::DbGatewayError> {
        Err(crate::DbGatewayError::Execution("unused".into()))
    }
}

struct CompiledCheckedResolver {
    binding: RegistryBinding,
    expected_signature: Vec<u8>,
}

#[async_trait]
impl StoredCallResolver for CompiledCheckedResolver {
    async fn resolve(
        &self,
        name: &str,
        _parent: &FnCtx,
    ) -> Result<ResolvedStoredCall, FunctionError> {
        Err(FunctionError::NotFound(name.to_string()))
    }

    async fn resolve_checked(
        &self,
        name: &str,
        parent: &FnCtx,
        expected_signature: &[u8],
    ) -> Result<ResolvedStoredCall, FunctionError> {
        assert_eq!(name, "target");
        assert_eq!(expected_signature, self.expected_signature.as_slice());
        Ok(ResolvedStoredCall::new(&self.binding, parent.clone(), None))
    }
}

async fn compile_procedure(
    engine: &Arc<WasmEngine>,
    source: &str,
    callable_catalog: std::collections::BTreeMap<String, Vec<u8>>,
) -> Arc<WasmFunction> {
    let options = CompileOptions {
        stage: ShamirStage::Procedures,
        callable_catalog,
        ..CompileOptions::default()
    };
    let module = shamir_lang::compile(source, &options).expect("procedure source compiles");
    let (function, _report) = WasmFunction::from_binary_admitted_blocking(
        engine.clone(),
        &module.wasm,
        WasmLimits::default(),
        TierMode::Auto,
        AdmissionPolicy::Load,
    )
    .await
    .expect("compiled procedure is admitted and loaded");
    Arc::new(function)
}

fn poll_until_db_gate<F>(future: &mut F, db: &CheckedCallDb, observer: &FuelObserver)
where
    F: Future<Output = Result<QueryValue, FunctionError>> + Unpin,
{
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    for _ in 0..100_000 {
        match Pin::new(&mut *future).poll(&mut context) {
            Poll::Pending => {}
            Poll::Ready(result) => panic!("compiled checked call finished before gate: {result:?}"),
        }
        if db.hit.load(Ordering::SeqCst) && observer.gate_observed() > 0 {
            return;
        }
    }
    panic!("compiled checked call did not reach gated db_get");
}

#[tokio::test]
async fn compiled_checked_call_cancellation_settles_fuel_and_allows_reentry() {
    let engine = Arc::new(WasmEngine::new().expect("engine"));
    let target = compile_procedure(
        &engine,
        "@entry procedure target() -> i64 { let row: { id: i64 }? = db_get(\"items\", 7); if (is_null(row)) { return 0; } return unwrap(row).id; }",
        std::collections::BTreeMap::new(),
    )
    .await;
    let expected_signature = b"procedure()->i64".to_vec();
    let mut catalog = std::collections::BTreeMap::new();
    catalog.insert("target".to_string(), expected_signature.clone());
    let caller = compile_procedure(
        &engine,
        "@entry procedure caller() -> i64 { return call(\"target\", {}); }",
        catalog,
    )
    .await;

    let registry = Arc::new(FunctionRegistry::new());
    registry.register("caller", caller).unwrap();
    registry.register("target", target).unwrap();
    let resolver = Arc::new(CompiledCheckedResolver {
        binding: registry
            .capture("target")
            .expect("captured compiled target"),
        expected_signature,
    });
    let db = Arc::new(CheckedCallDb {
        hit: AtomicBool::new(false),
    });
    let budget = Arc::new(AtomicU64::new(COMPILED_TEST_BUDGET));
    let observer = FuelObserver::new();
    let context = FnCtx::new()
        .with_registry(registry.clone())
        .with_stored_call_resolver(resolver)
        .with_db(db.clone(), "repo".to_string())
        .with_fuel_budget(budget.clone())
        .with_fuel_observer(observer.clone());
    let batch = FnBatch::new();
    let params = Params::new();

    let mut invocation = Box::pin(registry.invoke("caller", &context, &batch, &params));
    poll_until_db_gate(&mut invocation, &db, &observer);
    assert_eq!(observer.suspends(), 1);
    assert_eq!(observer.stores_with_fuel(), 1);
    let parent_grant = COMPILED_TEST_BUDGET;
    let parent_spent = parent_grant - observer.suspended_fuel_last();
    let child_grant = observer.last_install();
    assert_eq!(
        budget.load(Ordering::Relaxed),
        COMPILED_TEST_BUDGET - parent_spent - child_grant,
        "one active child lease remains while the parent has been revoked"
    );

    drop(invocation);
    assert_eq!(observer.settles(), 2);
    assert_eq!(
        budget.load(Ordering::Relaxed),
        COMPILED_TEST_BUDGET - observer.burned()
    );
    assert_eq!(observer.stores_with_fuel(), 0);
    observer
        .check_settled()
        .expect("both compiled Stores settle with no fuel installed");

    assert_eq!(
        registry
            .invoke("caller", &context, &batch, &params)
            .await
            .expect("the same captured compiled target succeeds after cancellation"),
        QueryValue::Int(7)
    );
    assert_eq!(context.active_call_path().len(), 0);
}

#[tokio::test]
async fn compiled_checked_call_projects_callee_result_and_rejects_wrong_shape() {
    let engine = Arc::new(WasmEngine::new().expect("engine"));
    let target = compile_procedure(
        &engine,
        "@entry procedure target() -> string { return \"not a record\"; }",
        std::collections::BTreeMap::new(),
    )
    .await;
    let expected_signature = b"procedure()->{id:i64}".to_vec();
    let mut catalog = std::collections::BTreeMap::new();
    catalog.insert("target".to_string(), expected_signature.clone());
    let caller = compile_procedure(
        &engine,
        "@entry procedure caller() -> { id: i64 } { return call(\"target\", {}); }",
        catalog,
    )
    .await;
    let registry = Arc::new(FunctionRegistry::new());
    registry.register("caller", caller).unwrap();
    registry.register("target", target).unwrap();
    let resolver = Arc::new(CompiledCheckedResolver {
        binding: registry
            .capture("target")
            .expect("captured compiled target"),
        expected_signature,
    });
    let context = FnCtx::new()
        .with_registry(registry.clone())
        .with_stored_call_resolver(resolver);
    let error = registry
        .invoke("caller", &context, &FnBatch::new(), &Params::new())
        .await
        .expect_err("a string cannot be projected as the declared record result");
    assert!(
        matches!(&error, FunctionError::Compute(message) if message.contains("guest codec:")),
        "compiled result projection must report guest kind 3, got {error:?}"
    );
}

#[tokio::test]
async fn checked_call_rejects_oversized_signature_before_guest_memory_read() {
    let signature_limit = CompileLimits::default().max_static_bytes;
    let oversized_len = i32::try_from(signature_limit + 1).expect("signature length fits i32");
    let caller_wat = format!(
        r#"(module
            (import "shamir_host" "call_checked"
                (func $checked (param i32 i32 i32 i32 i32 i32) (result i64)))
            (memory (export "memory") 1)
            (data (i32.const 0) "target")
            (data (i32.const 16) "\80")
            (func (export "shamir_alloc") (param i32) (result i32) i32.const 1024)
            (func (export "shamir_call") (param i32 i32) (result i64)
                (call $checked
                    (i32.const 0) (i32.const 6)
                    (i32.const 2147483647) (i32.const {oversized_len})
                    (i32.const 16) (i32.const 1)))
        )"#
    );
    let engine = Arc::new(WasmEngine::new().expect("engine"));
    let caller = WasmFunction::from_wat(engine, &caller_wat, WasmLimits::default())
        .expect("raw six-i32 checked-call module loads");
    let target_entered = Arc::new(AtomicBool::new(false));
    let registry = Arc::new(FunctionRegistry::new());
    registry.register("caller", Arc::new(caller)).unwrap();
    registry
        .register(
            "target",
            Arc::new(EnteredProbe(Arc::clone(&target_entered))),
        )
        .unwrap();
    let resolver = Arc::new(CapturedCheckedResolver {
        binding: registry.capture("target").expect("captured target binding"),
    });
    let context = FnCtx::new()
        .with_registry(Arc::clone(&registry))
        .with_stored_call_resolver(resolver);

    let error = registry
        .invoke("caller", &context, &FnBatch::new(), &Params::new())
        .await
        .expect_err("oversized signature is rejected before copying guest memory");
    let expected_marker = format!(
        "call_checked: signature length limit exceeded: {oversized_len} bytes > {signature_limit} bytes"
    );
    assert!(
        matches!(&error, FunctionError::Compute(message) if message.contains(&expected_marker)),
        "expected exact signature-length limit marker, got {error}"
    );
    assert!(
        !target_entered.load(Ordering::SeqCst),
        "oversized signature must be rejected before target dispatch"
    );
}

#[tokio::test]
async fn compiled_checked_call_treats_zero_host_result_as_kind_four_failure() {
    let engine = Arc::new(WasmEngine::new().expect("engine"));
    let mut catalog = std::collections::BTreeMap::new();
    catalog.insert("target".to_string(), b"procedure()->i64?".to_vec());
    let module = shamir_lang::compile(
        "@entry procedure caller() -> i64? { return call(\"target\", {}); }",
        &CompileOptions {
            stage: ShamirStage::Procedures,
            callable_catalog: catalog,
            ..CompileOptions::default()
        },
    )
    .expect("compiled caller source");
    let wasm_module =
        wasmtime::Module::from_binary(engine.engine(), &module.wasm).expect("compiled Wasm parses");
    let raised_kind = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let mut linker = wasmtime::Linker::<()>::new(engine.engine());
    linker
        .func_wrap(
            "shamir_host",
            "call_checked",
            |_: i32, _: i32, _: i32, _: i32, _: i32, _: i32| -> i64 { 0 },
        )
        .expect("test checked-call shim links");
    let observed_kind = Arc::clone(&raised_kind);
    linker
        .func_wrap(
            "shamir_host",
            "raise_error",
            move |kind: i32, _ptr: i32, _len: i32| -> Result<(), wasmtime::Error> {
                observed_kind.store(kind, Ordering::SeqCst);
                Err(wasmtime::Error::msg("guest raised an error"))
            },
        )
        .expect("test error shim links");
    let mut store = wasmtime::Store::new(engine.engine(), ());
    store
        .set_fuel(COMPILED_TEST_BUDGET)
        .expect("test Store fuel");
    store.set_epoch_deadline(1_000_000);
    let instance = linker
        .instantiate_async(&mut store, &wasm_module)
        .await
        .expect("compiled caller instantiates with test shims");
    let memory = instance
        .get_memory(&mut store, "memory")
        .expect("memory export");
    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, "shamir_alloc")
        .expect("allocator export");
    let call = instance
        .get_typed_func::<(i32, i32), i64>(&mut store, "shamir_call")
        .expect("call export");
    let input = crate::wasm::encode_params(&Params::new()).expect("empty params encode");
    let input_len = i32::try_from(input.len()).expect("params fit i32");
    let input_ptr = alloc
        .call_async(&mut store, input_len)
        .await
        .expect("guest allocates params");
    let input_start = usize::try_from(input_ptr).expect("valid guest input pointer");
    memory.data_mut(&mut store)[input_start..input_start + input.len()].copy_from_slice(&input);

    let result = call.call_async(&mut store, (input_ptr, input_len)).await;
    assert!(result.is_err(), "zero is not accepted as a nullable result");
    assert_eq!(
        raised_kind.load(Ordering::SeqCst),
        4,
        "zero checked-call result must raise the guest host-contract error kind"
    );
}
