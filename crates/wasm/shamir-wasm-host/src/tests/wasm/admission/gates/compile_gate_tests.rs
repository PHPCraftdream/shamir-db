//! Compile gate: the semaphore bounds concurrent compiles, the time cap rejects slow admitted compiles while the permit stays held until the thread ends, and Load has no cap.

use crate::tests::wasm::admission::support::{limits_with, module};
use crate::{
    AdmissionLimits, AdmissionPolicy, FnResult, FunctionError, ModuleReport, TierMode, WasmEngine,
    WasmEngineOptions, WasmFunction, WasmLimits,
};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

async fn admitted_compile(
    engine: &Arc<WasmEngine>,
    policy: AdmissionPolicy,
) -> FnResult<(WasmFunction, ModuleReport)> {
    WasmFunction::from_binary_admitted_blocking(
        Arc::clone(engine),
        &module(""),
        WasmLimits::default(),
        TierMode::Auto,
        policy,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_permit_serializes_blocking_sections() {
    let engine = engine_with_gate(1);
    engine
        .faults()
        .compile_delay_ms
        .store(150, Ordering::SeqCst);
    let (first, second) = tokio::join!(
        admitted_compile(
            &engine,
            AdmissionPolicy::Admission(AdmissionLimits::default())
        ),
        admitted_compile(
            &engine,
            AdmissionPolicy::Admission(AdmissionLimits::default())
        ),
    );
    assert!(first.is_ok());
    assert!(second.is_ok());
    assert_eq!(engine.faults().compile_max_active.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_permits_overlap_blocking_sections() {
    let engine = engine_with_gate(2);
    engine
        .faults()
        .compile_delay_ms
        .store(300, Ordering::SeqCst);
    let (first, second) = tokio::join!(
        admitted_compile(
            &engine,
            AdmissionPolicy::Admission(AdmissionLimits::default())
        ),
        admitted_compile(
            &engine,
            AdmissionPolicy::Admission(AdmissionLimits::default())
        ),
    );
    assert!(first.is_ok());
    assert!(second.is_ok());
    assert_eq!(engine.faults().compile_max_active.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn time_cap_rejects_and_the_permit_outlives_the_timeout() {
    let engine = engine_with_gate(1);
    engine
        .faults()
        .compile_delay_ms
        .store(600, Ordering::SeqCst);
    let policy = AdmissionPolicy::Admission(limits_with(|l| {
        l.max_compile_time = Duration::from_millis(150);
    }));
    let err = admitted_compile(&engine, policy)
        .await
        .err()
        .expect("must time out");
    assert!(
        matches!(&err, FunctionError::Compute(m) if m.contains("compile time cap") && m.contains("rejected")),
        "{err}"
    );
    engine.faults().compile_delay_ms.store(0, Ordering::SeqCst);
    let started = Instant::now();
    assert!(admitted_compile(&engine, AdmissionPolicy::Load)
        .await
        .is_ok());
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "the second compile must have waited for the first thread's permit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn load_policy_ignores_the_time_cap() {
    let engine = Arc::new(
        WasmEngine::with_options(WasmEngineOptions {
            admission: AdmissionLimits {
                max_compile_time: Duration::from_millis(50),
                ..AdmissionLimits::default()
            },
            ..Default::default()
        })
        .unwrap(),
    );
    engine
        .faults()
        .compile_delay_ms
        .store(300, Ordering::SeqCst);
    assert!(admitted_compile(&engine, AdmissionPolicy::Load)
        .await
        .is_ok());
}
