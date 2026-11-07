//! WasmFunction::tier_report / mode: tier, configured mode, saturating call counter and upgrade state per function.

use crate::tests::wasm::engine::support::{
    auto_engine, call_once, echo_params, identity_wat, wait_for_tier, wait_until, HOT_CALLS,
};
use crate::{Tier, TierMode, TierReport, UpgradeState, WasmFunction, WasmLimits};

fn function_with(mode: TierMode) -> WasmFunction {
    WasmFunction::from_wat_with_mode(auto_engine(), &identity_wat(), WasmLimits::default(), mode)
        .unwrap()
}

fn report(tier: Tier, mode: TierMode, calls_observed: u64, state: UpgradeState) -> TierReport {
    TierReport {
        tier,
        mode,
        calls_observed,
        upgrade_state: state,
    }
}

#[tokio::test]
async fn fresh_auto_function_reports_fast_idle() {
    let function = function_with(TierMode::Auto);
    assert_eq!(
        function.tier_report(),
        report(Tier::Fast, TierMode::Auto, 0, UpgradeState::Idle)
    );
    assert_eq!(function.mode(), TierMode::Auto);
}

#[tokio::test]
async fn auto_function_reports_optimized_after_the_upgrade() {
    let function = function_with(TierMode::Auto);
    let params = echo_params();
    call_once(&function, &params).await;
    assert_eq!(
        function.tier_report(),
        report(Tier::Fast, TierMode::Auto, 1, UpgradeState::Idle)
    );
    for _ in 1..HOT_CALLS {
        call_once(&function, &params).await;
    }
    wait_for_tier(&function, Tier::Optimized).await;
    wait_until("upgrade state never reached Done", || {
        function.tier_report().upgrade_state == UpgradeState::Done
    })
    .await;
    for _ in 0..5 {
        call_once(&function, &params).await;
    }
    assert_eq!(
        function.tier_report(),
        report(
            Tier::Optimized,
            TierMode::Auto,
            HOT_CALLS,
            UpgradeState::Done
        )
    );
}

#[tokio::test]
async fn never_stays_fast_and_counts_nothing() {
    let function = function_with(TierMode::Never);
    let params = echo_params();
    for _ in 0..3 * HOT_CALLS {
        call_once(&function, &params).await;
    }
    assert_eq!(
        function.tier_report(),
        report(Tier::Fast, TierMode::Never, 0, UpgradeState::Idle)
    );
}

#[tokio::test]
async fn now_is_optimized_from_the_start() {
    let function = function_with(TierMode::Now);
    let expected = report(Tier::Optimized, TierMode::Now, 0, UpgradeState::Idle);
    assert_eq!(function.tier_report(), expected);
    let params = echo_params();
    for _ in 0..HOT_CALLS {
        call_once(&function, &params).await;
    }
    assert_eq!(function.tier_report(), expected);
}
