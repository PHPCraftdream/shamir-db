//! Tier parity of the interruption limits: fuel exhaustion and epoch
//! interruption trap IDENTICALLY on the fast (`Never`) and the optimized
//! (`Now`) tier, and — with `Auto` — on BOTH slots across the upgrade,
//! proving both engines' epoch tickers advance.

use super::support::{assert_fuel_typed, INFINITE_LOOP_WAT};
use crate::{
    FnBatch, FnCtx, FunctionError, Params, ShamirFunction, Tier, TierMode, TierOptions, WasmEngine,
    WasmEngineOptions, WasmFunction, WasmLimits,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{sleep, timeout};

fn tiered_engine() -> Arc<WasmEngine> {
    Arc::new(
        WasmEngine::with_options(WasmEngineOptions {
            tier: TierOptions {
                enabled: true,
                hot_calls: 3,
                background_jobs: 2,
            },
            ..Default::default()
        })
        .expect("tiered engine must build"),
    )
}

/// A spinning guest with explicit fuel and wall-clock budgets, created in
/// `mode` (fuel helpers: `support::INFINITE_LOOP_WAT`).
fn spinning(
    engine: &Arc<WasmEngine>,
    mode: TierMode,
    fuel: u64,
    deadline: Duration,
) -> Arc<WasmFunction> {
    Arc::new(
        WasmFunction::from_wat_with_mode(
            Arc::clone(engine),
            INFINITE_LOOP_WAT,
            WasmLimits {
                fuel,
                wall_clock_deadline: deadline,
                ..Default::default()
            },
            mode,
        )
        .expect("spinning guest must compile"),
    )
}

fn expected_tier(mode: TierMode) -> Tier {
    match mode {
        TierMode::Now => Tier::Optimized,
        TierMode::Auto | TierMode::Never => Tier::Fast,
    }
}

async fn call_error(function: &WasmFunction) -> FunctionError {
    function
        .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
        .await
        .expect_err("the spinning guest must be interrupted, not finish")
}

/// Case 10 (fuel) — a spinning guest with a tiny fuel budget traps with the
/// typed `FuelExhausted` on BOTH tiers (`Never` fast slot, `Now` optimized
/// slot): identical classification.
#[tokio::test]
async fn fuel_exhaustion_traps_identically_on_both_tiers() {
    let engine = tiered_engine();
    for mode in [TierMode::Never, TierMode::Now] {
        let function = spinning(&engine, mode, 1_000, Duration::from_secs(30));
        assert_eq!(
            function.tier(),
            expected_tier(mode),
            "{mode:?} created on the wrong tier"
        );
        let error = call_error(&function).await;
        assert_fuel_typed(error);
    }
}

/// Case 10 (epoch) — a spinning guest with a huge fuel budget and a tiny
/// wall-clock deadline is interrupted on BOTH tiers with the SAME typed
/// `Interrupted` error: the per-Store epoch deadline fires (the guest never
/// yields, so the top-level `timeout` cannot fire first).
#[tokio::test]
async fn epoch_deadline_interrupts_identically_on_both_tiers() {
    let engine = tiered_engine();
    for mode in [TierMode::Never, TierMode::Now] {
        let function = spinning(&engine, mode, u64::MAX, Duration::from_millis(300));
        assert_eq!(
            function.tier(),
            expected_tier(mode),
            "{mode:?} created on the wrong tier"
        );
        let started = Instant::now();
        let error = call_error(&function).await;
        assert!(
            matches!(error, FunctionError::Interrupted),
            "expected typed Interrupted on {mode:?}, got: {error:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the {mode:?} guest ran {:?}; the epoch deadline did not interrupt it",
            started.elapsed()
        );
    }
}

/// Case 11 — BOTH engines' epochs advance: with `Auto`, the wall-clock
/// deadline interrupts the spinning guest on the fast slot BEFORE the
/// upgrade and on the optimized slot AFTER it.
#[tokio::test]
async fn both_engines_epoch_deadline_interrupts_across_the_upgrade() {
    let engine = tiered_engine();
    let function = spinning(
        &engine,
        TierMode::Auto,
        u64::MAX,
        Duration::from_millis(300),
    );
    assert_eq!(function.tier(), Tier::Fast);

    let error = call_error(&function).await;
    assert!(
        matches!(error, FunctionError::Interrupted),
        "the fast slot's epoch deadline did not interrupt: {error:?}"
    );

    // Keep calling (each call is interrupted after about one deadline; the
    // call counter still rises) until the upgrade installs the optimized
    // slot — bounded, never an unbounded loop.
    timeout(Duration::from_secs(30), async {
        while function.tier() == Tier::Fast {
            let _ = function
                .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
                .await;
            // Yield so the scheduled upgrade job can take its permit and compile.
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the upgrade never completed");

    let error = call_error(&function).await;
    assert!(
        matches!(error, FunctionError::Interrupted),
        "the optimized slot's epoch deadline did not interrupt: {error:?}"
    );
}
