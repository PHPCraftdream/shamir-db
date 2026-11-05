//! Creation-time tier modes: `Never` stays on the fast tier and schedules
//! nothing, `Now` starts on the optimized tier, and a tiers-disabled engine
//! hosts BOTH `Auto` and `Never` on the optimized tier (the historical
//! defect: they landed on a fast tier).

use super::support::{auto_engine, call_once, echo_params, identity_wat, HOT_CALLS};
use crate::{Tier, TierMode, TierOptions, WasmEngine, WasmEngineOptions, WasmFunction, WasmLimits};
use std::sync::atomic::Ordering;
use std::sync::Arc;

fn function_with_mode(engine: &Arc<WasmEngine>, mode: TierMode) -> Arc<WasmFunction> {
    Arc::new(
        WasmFunction::from_wat_with_mode(
            Arc::clone(engine),
            &identity_wat(),
            WasmLimits::default(),
            mode,
        )
        .expect("identity guest must compile"),
    )
}

/// Case 5 — `TierMode::Never` stays `Fast` far past `hot_calls` and starts
/// no upgrade.
#[tokio::test]
async fn tier_mode_never_stays_fast_and_starts_no_upgrade() {
    let engine = auto_engine();
    let function = function_with_mode(&engine, TierMode::Never);
    let stats = engine.tier_stats();
    assert_eq!(function.tier(), Tier::Fast);

    for _ in 0..(5 * HOT_CALLS) {
        call_once(&function, &echo_params()).await;
    }

    assert_eq!(
        function.tier(),
        Tier::Fast,
        "TierMode::Never left the fast tier"
    );
    assert_eq!(
        stats.upgrades_started.load(Ordering::Relaxed),
        0,
        "TierMode::Never scheduled an upgrade"
    );
}

/// Case 6 — `TierMode::Now` is `Optimized` right after creation and starts
/// no upgrade.
#[tokio::test]
async fn tier_mode_now_starts_optimized_and_starts_no_upgrade() {
    let engine = auto_engine();
    let function = function_with_mode(&engine, TierMode::Now);
    let stats = engine.tier_stats();
    assert_eq!(
        function.tier(),
        Tier::Optimized,
        "TierMode::Now did not start on the optimized tier"
    );

    for _ in 0..(2 * HOT_CALLS) {
        call_once(&function, &echo_params()).await;
    }

    assert_eq!(function.tier(), Tier::Optimized);
    assert_eq!(
        stats.upgrades_started.load(Ordering::Relaxed),
        0,
        "TierMode::Now scheduled an upgrade"
    );
    assert_eq!(stats.upgrades_completed.load(Ordering::Relaxed), 0);
}

/// Case 7 — `TierOptions::enabled == false`: the engine has no fast tier, so
/// BOTH `Auto` and `Never` functions are created on the optimized tier and
/// never upgrade. This pins a real defect: without the fix both modes
/// landed on a fast tier instead.
#[tokio::test]
async fn tiers_disabled_creates_auto_and_never_on_the_optimized_tier() {
    let engine = Arc::new(
        WasmEngine::with_options(WasmEngineOptions {
            tier: TierOptions {
                enabled: false,
                hot_calls: HOT_CALLS,
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("single-tier engine must build"),
    );
    assert!(
        engine.fast_engine().is_none(),
        "a tiers-disabled engine must have no fast engine"
    );
    let stats = engine.tier_stats();

    for mode in [TierMode::Auto, TierMode::Never] {
        let function = function_with_mode(&engine, mode);
        assert_eq!(
            function.tier(),
            Tier::Optimized,
            "{mode:?} must be created on the optimized tier when tiers are disabled"
        );
        for _ in 0..(3 * HOT_CALLS) {
            call_once(&function, &echo_params()).await;
        }
        assert_eq!(
            function.tier(),
            Tier::Optimized,
            "{mode:?} upgraded although tiers are disabled"
        );
    }
    assert_eq!(
        stats.upgrades_started.load(Ordering::Relaxed),
        0,
        "a tiers-disabled engine scheduled an upgrade"
    );
}
