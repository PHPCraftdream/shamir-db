//! check_module: admits and compiles exactly as creation would for the tier mode, persists nothing and never writes the incremental disk compile cache.

use crate::tests::wasm::admission::support::{
    admit, assemble, limits_with, module, module_with_call_body,
};
use crate::{
    check_module, AdmissionLimits, AdmissionPolicy, FnResult, FunctionError, ModuleHash,
    ModuleReport, TierMode, TierOptions, WasmEngine, WasmEngineOptions,
};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

fn engine_with_tiers(enabled: bool) -> Arc<WasmEngine> {
    Arc::new(
        WasmEngine::with_options(WasmEngineOptions {
            tier: TierOptions {
                enabled,
                ..TierOptions::default()
            },
            ..Default::default()
        })
        .unwrap(),
    )
}

async fn check(engine: &Arc<WasmEngine>, bytes: &[u8], mode: TierMode) -> FnResult<ModuleReport> {
    check_module(
        Arc::clone(engine),
        bytes,
        AdmissionPolicy::Admission(AdmissionLimits::default()),
        mode,
    )
    .await
}

fn count_cache_entries(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                count_cache_entries(&path)
            } else {
                usize::from(path.extension().is_some_and(|ext| ext == "e"))
            }
        })
        .sum()
}

#[tokio::test]
async fn auto_accepts_a_normal_module_and_reports_its_hash() {
    let engine = engine_with_tiers(true);
    let bytes = module("");
    let report = check(&engine, &bytes, TierMode::Auto).await.unwrap();
    assert_eq!(report.hash, ModuleHash::of(&bytes));
    assert_eq!(report.byte_size, bytes.len());
    assert_eq!(report, admit(&bytes, AdmissionLimits::default()).unwrap());
}

/// The fast-compile fault seam stands in for a module the Winch backend rejects.
#[tokio::test]
async fn never_rejects_when_the_fast_tier_rejects_and_auto_falls_back() {
    let engine = engine_with_tiers(true);
    engine
        .faults()
        .fail_fast_compile
        .store(true, Ordering::SeqCst);
    let bytes = module("");
    let err = check(&engine, &bytes, TierMode::Never)
        .await
        .expect_err("Never must reject");
    assert!(err.to_string().contains("WASM compile error"), "{err}");
    assert!(check(&engine, &bytes, TierMode::Auto).await.is_ok());
    assert!(check(&engine, &bytes, TierMode::Now).await.is_ok());
}

#[tokio::test]
async fn never_without_a_fast_engine_uses_the_optimized_tier() {
    let engine = engine_with_tiers(false);
    assert!(engine.fast_engine().is_none());
    let bytes = module("");
    assert!(check(&engine, &bytes, TierMode::Never).await.is_ok());
    assert!(check(&engine, &bytes, TierMode::Auto).await.is_ok());
    assert!(check(&engine, &bytes, TierMode::Now).await.is_ok());
}

/// The fail-fast seam would fail any fast-tier compile, so `Now` passing proves it skips that tier.
#[tokio::test]
async fn now_does_not_touch_the_fast_tier() {
    let engine = engine_with_tiers(true);
    engine
        .faults()
        .fail_fast_compile
        .store(true, Ordering::SeqCst);
    assert!(check(&engine, &module(""), TierMode::Now).await.is_ok());
}

#[tokio::test]
async fn check_never_writes_the_disk_cache() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        WasmEngine::with_compile_cache_and_options(
            dir.path().to_path_buf(),
            WasmEngineOptions::default(),
        )
        .unwrap(),
    );
    let bytes = module("");
    assert!(check(&engine, &bytes, TierMode::Auto).await.is_ok());
    assert!(check(&engine, &bytes, TierMode::Now).await.is_ok());
    assert!(check(&engine, &bytes, TierMode::Never).await.is_ok());
    engine
        .faults()
        .fail_fast_compile
        .store(true, Ordering::SeqCst);
    assert!(check(&engine, &bytes, TierMode::Auto).await.is_ok());
    let cache = dir.path().join("cache");
    assert_eq!(count_cache_entries(&cache), 0);
    assert_eq!(
        engine.tier_stats().upgrades_started.load(Ordering::SeqCst),
        0
    );
    engine.compile_optimized(&bytes).unwrap();
    assert!(
        count_cache_entries(&cache) > 0,
        "the harness must see an entry written by a normal compile, otherwise the zero above proves nothing"
    );
}

#[tokio::test]
async fn admission_failures_surface_unchanged() {
    let engine = engine_with_tiers(true);
    let cases = [
        (
            module(""),
            limits_with(|l| l.max_operators_per_function = 3),
        ),
        (assemble("(module)"), AdmissionLimits::default()),
        (
            module_with_call_body("", "(drop (v128.const i32x4 0 0 0 0))"),
            AdmissionLimits::default(),
        ),
        (module(""), limits_with(|l| l.max_module_bytes = 8)),
    ];
    for (index, (bytes, limits)) in cases.into_iter().enumerate() {
        let expected = admit(&bytes, limits).unwrap_err();
        let actual = check_module(
            Arc::clone(&engine),
            &bytes,
            AdmissionPolicy::Admission(limits),
            TierMode::Auto,
        )
        .await
        .expect_err("admission must fail");
        assert!(
            matches!(&actual, FunctionError::Compute(m) if *m == expected.to_string()),
            "case {index}: {actual} vs {expected}"
        );
    }
}
