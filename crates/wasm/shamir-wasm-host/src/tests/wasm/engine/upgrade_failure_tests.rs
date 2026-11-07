//! Tier failure behaviour: a failing CREATION-time fast compile silently
//! falls back to the optimized tier, and a failing or PANICKING background
//! upgrade keeps the function working, counts `upgrades_failed`, retries
//! once at 16x `hot_calls`, and never wedges the state machine.
//!
//! Both injections go through the `#[cfg(test)]` `TierFaults` seam on
//! `WasmEngine` (see `wasm_engine.rs`): a module the Winch backend rejects
//! could not be verified without running the suite, and an upgrade compile
//! cannot be made to fail from the outside at all.

use super::support::{
    auto_engine, call_once, echo_params, echo_result, identity_wat, wait_until, HOT_CALLS,
};
use crate::wasm::TierFaults;
use crate::{Tier, TierMode, WasmEngine, WasmFunction, WasmLimits};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

/// An `Auto`-mode identity function on `engine`.
fn auto_identity(engine: &Arc<WasmEngine>) -> Arc<WasmFunction> {
    Arc::new(
        WasmFunction::from_wat_with_mode(
            Arc::clone(engine),
            &identity_wat(),
            WasmLimits::default(),
            TierMode::Auto,
        )
        .expect("identity guest must compile"),
    )
}

/// Case 8 — an `Auto` creation whose FAST-tier compile fails falls back to
/// the optimized tier: the caller gets a working `Tier::Optimized` function,
/// not an error, and the pre-optimized slot never schedules an upgrade.
#[tokio::test]
async fn fast_tier_compile_failure_falls_back_to_the_optimized_tier() {
    let engine = auto_engine();
    assert!(
        engine.fast_engine().is_some(),
        "the fast engine must exist so the injected compile failure is exercised"
    );
    engine
        .faults()
        .fail_fast_compile
        .store(true, Ordering::Relaxed);

    let function = Arc::new(
        WasmFunction::from_wat_with_mode(
            Arc::clone(&engine),
            &identity_wat(),
            WasmLimits::default(),
            TierMode::Auto,
        )
        .expect("the fast-compile failure must fall back, not surface as an error"),
    );
    assert_eq!(
        function.tier(),
        Tier::Optimized,
        "the slot must fall back to the optimized tier"
    );

    // The slot is already optimized: crossing the threshold retires the
    // upgrade machine without scheduling anything.
    for _ in 0..(3 * HOT_CALLS) {
        call_once(&function, &echo_params()).await;
    }
    sleep(Duration::from_millis(50)).await;
    assert_eq!(
        engine.tier_stats().upgrades_started.load(Ordering::Relaxed),
        0,
        "a pre-optimized slot must never schedule an upgrade"
    );
    let params = echo_params();
    assert_eq!(call_once(&function, &params).await, echo_result(&params));
}

/// Case 9 (error) — the background upgrade fails: `upgrades_failed` counts
/// it and the fast slot keeps serving correct results.
#[tokio::test]
async fn failed_upgrade_keeps_the_fast_slot_working_and_counts_the_failure() {
    let engine = auto_engine();
    let function = auto_identity(&engine);
    engine
        .faults()
        .upgrade_fault
        .store(TierFaults::UPGRADE_FAIL, Ordering::Relaxed);

    for _ in 0..HOT_CALLS {
        call_once(&function, &echo_params()).await;
    }
    let stats = engine.tier_stats();
    wait_until("the failed upgrade was never counted", || {
        stats.upgrades_failed.load(Ordering::Relaxed) == 1
    })
    .await;

    assert_eq!(
        function.tier(),
        Tier::Fast,
        "a failed upgrade must keep the fast slot"
    );
    assert_eq!(stats.upgrades_completed.load(Ordering::Relaxed), 0);
    let params = echo_params();
    assert_eq!(
        call_once(&function, &params).await,
        echo_result(&params),
        "the function must keep working after a failed upgrade"
    );
}

/// Case 9 (panic) — a PANICKING upgrade counts as failed, and the state
/// machine retries once on the next threshold crossing at `16 x hot_calls`.
/// The retry fails as well, the machine is exhausted, further crossings
/// schedule nothing, and the function keeps working on the fast tier
/// throughout.
#[tokio::test]
async fn panicking_upgrade_is_counted_failed_and_retries_once_at_16x_hot_calls() {
    let engine = auto_engine();
    let function = auto_identity(&engine);
    engine
        .faults()
        .upgrade_fault
        .store(TierFaults::UPGRADE_PANIC, Ordering::Relaxed);
    let stats = engine.tier_stats();

    // First attempt (crosses `hot_calls`): panics, counted as failed.
    for _ in 0..HOT_CALLS {
        call_once(&function, &echo_params()).await;
    }
    wait_until("the panicking upgrade was never counted", || {
        stats.upgrades_failed.load(Ordering::Relaxed) == 1
    })
    .await;
    assert_eq!(function.tier(), Tier::Fast);

    // One retry on the next threshold crossing at 16x `hot_calls` — the
    // calls counter keeps rising while the machine is in `Failed`. One call
    // short of the crossing nothing may start...
    for _ in 0..(15 * HOT_CALLS - 1) {
        call_once(&function, &echo_params()).await;
    }
    sleep(Duration::from_millis(100)).await;
    assert_eq!(
        stats.upgrades_started.load(Ordering::Relaxed),
        1,
        "the retry started before 16x hot_calls"
    );
    // ...and the crossing call starts exactly the retry.
    call_once(&function, &echo_params()).await;
    wait_until("the 16x-threshold retry never started", || {
        stats.upgrades_started.load(Ordering::Relaxed) == 2
    })
    .await;
    wait_until("the retry's panic was never counted", || {
        stats.upgrades_failed.load(Ordering::Relaxed) == 2
    })
    .await;

    // Exhausted: clearing the fault must not produce a third attempt.
    engine
        .faults()
        .upgrade_fault
        .store(TierFaults::UPGRADE_NONE, Ordering::Relaxed);
    for _ in 0..(16 * HOT_CALLS) {
        call_once(&function, &echo_params()).await;
    }
    sleep(Duration::from_millis(100)).await;
    assert_eq!(
        stats.upgrades_started.load(Ordering::Relaxed),
        2,
        "a third upgrade attempt was scheduled after exhaustion"
    );
    assert_eq!(
        function.tier(),
        Tier::Fast,
        "the slot must stay fast while every upgrade fails"
    );
    let params = echo_params();
    assert_eq!(call_once(&function, &params).await, echo_result(&params));
}
