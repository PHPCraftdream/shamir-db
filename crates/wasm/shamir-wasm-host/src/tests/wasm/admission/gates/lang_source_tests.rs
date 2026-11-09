//! shamir source service: compiles `shamir` source under exactly one compile-gate permit, renders compiler errors deterministically, and persists nothing on check.

use crate::{
    check_shamir_source, create_shamir_source, AdmissionLimits, CompileOptions, FnBatch, FnCtx,
    FunctionError, ModuleHash, Params, ShamirFunction, TierMode, WasmEngine, WasmEngineOptions,
    WasmLimits,
};
use shamir_types::types::common::TMap;
use shamir_types::types::value::QueryValue;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const SCALAR: &str = "@entry scalar double(x: i64) -> i64 { return x + x; }\n";

const VALIDATOR: &str = "@entry validator require_name(\n\
     record: { name: string }?,\n\
     old_record: { name: string }?\n\
     ) -> Validation {\n\
     if (is_null(record)) {\n\
     return record_error(\"record_required\");\n\
     }\n\
     return accept();\n\
     }\n";

fn engine_with_gate(max_concurrent: usize) -> Arc<WasmEngine> {
    Arc::new(
        WasmEngine::with_options(WasmEngineOptions {
            admission: AdmissionLimits {
                max_concurrent_compiles: max_concurrent,
                ..AdmissionLimits::default()
            },
            ..Default::default()
        })
        .unwrap(),
    )
}

fn x_params(value: i64) -> Params {
    let map: TMap<String, QueryValue> = [("x".to_string(), QueryValue::Int(value))]
        .into_iter()
        .collect();
    Params::from_map(map)
}

fn compute_message(error: FunctionError) -> String {
    match error {
        FunctionError::Compute(message) => message,
        other => panic!("expected FunctionError::Compute, got {other}"),
    }
}

#[tokio::test]
async fn create_scalar_runs_and_reports_consistently() {
    let engine = engine_with_gate(1);
    let (function, bytes, report) = create_shamir_source(
        Arc::clone(&engine),
        SCALAR.to_string(),
        CompileOptions::default(),
        WasmLimits::default(),
        TierMode::Auto,
    )
    .await
    .expect("scalar source compiles");
    // derivation: the report is the host-side scan of the returned bytes.
    assert_eq!(report.hash, ModuleHash::of(&bytes));
    assert_eq!(report.byte_size, bytes.len());
    // derivation: x + x with x = 21 is 42 through the named envelope.
    let result = function
        .call(&FnCtx::new(), &FnBatch::new(), &x_params(21))
        .await
        .expect("compiled scalar runs");
    assert_eq!(result, QueryValue::Int(42));
}

#[tokio::test]
async fn check_matches_create_for_every_tier_mode() {
    let engine = engine_with_gate(1);
    let (_function, _bytes, created) = create_shamir_source(
        Arc::clone(&engine),
        SCALAR.to_string(),
        CompileOptions::default(),
        WasmLimits::default(),
        TierMode::Auto,
    )
    .await
    .expect("scalar source compiles");
    for mode in [TierMode::Auto, TierMode::Now, TierMode::Never] {
        let checked = check_shamir_source(
            Arc::clone(&engine),
            SCALAR.to_string(),
            CompileOptions::default(),
            WasmLimits::default(),
            mode,
        )
        .await
        .expect("check accepts the same source");
        // derivation: compilation is deterministic, so the same source
        // yields the same module and therefore the same hash.
        assert_eq!(checked.hash, created.hash);
    }
}

#[tokio::test]
async fn validator_source_is_accepted_by_check() {
    let engine = engine_with_gate(1);
    let report = check_shamir_source(
        engine,
        VALIDATOR.to_string(),
        CompileOptions::default(),
        WasmLimits::default(),
        TierMode::Auto,
    )
    .await
    .expect("validator source passes the dry run");
    assert!(report.byte_size > 0);
}

#[tokio::test]
async fn compile_error_renders_deterministically_with_code_and_position() {
    let engine = engine_with_gate(1);
    let source = "@entry scalar x( -> i64 { }";
    let render = || async {
        let error = create_shamir_source(
            Arc::clone(&engine),
            source.to_string(),
            CompileOptions::default(),
            WasmLimits::default(),
            TierMode::Auto,
        )
        .await
        .err()
        .expect("broken source must fail");
        compute_message(error)
    };
    let first = render().await;
    let second = render().await;
    assert_eq!(first, second);
    // derivation: each diagnostic renders as `CODE:line:column: message`
    // and the broken token is on line 1.
    assert!(first.contains(":1:"), "{first}");
}

#[tokio::test]
async fn procedure_entries_are_rejected_with_t007() {
    let engine = engine_with_gate(1);
    let error = check_shamir_source(
        engine,
        "@entry procedure p() -> i64 { return 1; }\n".to_string(),
        CompileOptions::default(),
        WasmLimits::default(),
        TierMode::Auto,
    )
    .await
    .expect_err("procedures are not supported yet");
    assert!(compute_message(error).contains("T007"));
}

/// With ONE permit a nested gate acquisition would deadlock, and a leaked
/// permit would hang the second call; the timeout turns either into a
/// failure.
#[tokio::test]
async fn one_permit_is_released_after_success_error_and_check() {
    let engine = engine_with_gate(1);
    let run = async {
        let ok = create_shamir_source(
            Arc::clone(&engine),
            SCALAR.to_string(),
            CompileOptions::default(),
            WasmLimits::default(),
            TierMode::Auto,
        )
        .await;
        assert!(ok.is_ok());
        let failed = create_shamir_source(
            Arc::clone(&engine),
            "@entry scalar x( -> i64 { }".to_string(),
            CompileOptions::default(),
            WasmLimits::default(),
            TierMode::Auto,
        )
        .await;
        assert!(failed.is_err());
        let checked = check_shamir_source(
            Arc::clone(&engine),
            SCALAR.to_string(),
            CompileOptions::default(),
            WasmLimits::default(),
            TierMode::Auto,
        )
        .await;
        assert!(checked.is_ok());
    };
    tokio::time::timeout(Duration::from_secs(60), run)
        .await
        .expect("every call released its permit");
    // derivation: the compile probe counts blocking sections active at once;
    // one permit and no nested gates keeps the maximum at 1.
    assert_eq!(engine.faults().compile_max_active.load(Ordering::SeqCst), 1);
}
