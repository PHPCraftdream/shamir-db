//! Two-tier upgrade path: identical results across the slot swap, the
//! `Fast` → `Optimized` transition exactly at `hot_calls`, and exactly one
//! upgrade when many concurrent callers cross the threshold together.

use super::support::{
    auto_engine, call_once, echo_params, echo_result, identity_wat, int_sum_native, int_sum_wat,
    wait_for_tier, wait_until, HOT_CALLS,
};
use crate::{Params, Tier, TierMode, WasmEngine, WasmFunction, WasmLimits};
use shamir_types::types::value::QueryValue;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Barrier;
use tokio::time::sleep;

/// An `Auto`-mode function compiled from `wat` on the shared test engine.
fn auto_function(engine: &Arc<WasmEngine>, wat: &str) -> Arc<WasmFunction> {
    Arc::new(
        WasmFunction::from_wat_with_mode(
            Arc::clone(engine),
            wat,
            WasmLimits::default(),
            TierMode::Auto,
        )
        .expect("guest must compile"),
    )
}

/// Case 1 — results identical before and after the upgrade (identity kernel).
#[tokio::test]
async fn identity_results_are_identical_before_and_after_the_upgrade() {
    let engine = auto_engine();
    let function = auto_function(&engine, &identity_wat());
    let params = echo_params();
    let expected = echo_result(&params);
    assert_eq!(function.tier(), Tier::Fast);

    let before = call_once(&function, &params).await;
    assert_eq!(before, expected, "pre-upgrade result changed");
    for _ in 0..HOT_CALLS {
        call_once(&function, &params).await;
    }
    wait_for_tier(&function, Tier::Optimized).await;

    let after = call_once(&function, &params).await;
    assert_eq!(
        after, expected,
        "post-upgrade result differs from the pre-upgrade result"
    );
}

/// Case 1 — results identical before and after the upgrade (compute kernel).
#[tokio::test]
async fn compute_kernel_results_are_identical_before_and_after_the_upgrade() {
    let engine = auto_engine();
    let function = auto_function(&engine, &int_sum_wat());
    let expected = QueryValue::Int(i64::from(int_sum_native() & 127));
    assert_eq!(function.tier(), Tier::Fast);

    let before = call_once(&function, &Params::new()).await;
    assert_eq!(before, expected, "pre-upgrade kernel result changed");
    for _ in 0..HOT_CALLS {
        call_once(&function, &Params::new()).await;
    }
    wait_for_tier(&function, Tier::Optimized).await;

    let after = call_once(&function, &Params::new()).await;
    assert_eq!(after, expected, "post-upgrade kernel result differs");
}

/// Case 2 — the slot is `Fast` below the threshold and `Optimized` after it,
/// with exactly one completed upgrade in `TierStats`.
#[tokio::test]
async fn slot_is_fast_before_the_threshold_and_optimized_after_it() {
    let engine = auto_engine();
    let function = auto_function(&engine, &identity_wat());
    let stats = engine.tier_stats();
    assert_eq!(function.tier(), Tier::Fast);

    // `hot_calls - 1` calls stay below the threshold: no upgrade may start.
    for _ in 0..(HOT_CALLS - 1) {
        call_once(&function, &echo_params()).await;
    }
    sleep(Duration::from_millis(50)).await;
    assert_eq!(
        function.tier(),
        Tier::Fast,
        "the slot left the fast tier below the threshold"
    );
    assert_eq!(
        stats.upgrades_started.load(Ordering::Relaxed),
        0,
        "an upgrade started below the hot-calls threshold"
    );

    call_once(&function, &echo_params()).await; // the call that crosses the threshold
    wait_until("the threshold call never scheduled the upgrade", || {
        stats.upgrades_started.load(Ordering::Relaxed) == 1
    })
    .await;
    wait_for_tier(&function, Tier::Optimized).await;

    assert_eq!(
        stats.upgrades_completed.load(Ordering::Relaxed),
        1,
        "exactly one upgrade must complete"
    );
    assert_eq!(stats.upgrades_failed.load(Ordering::Relaxed), 0);
}

/// Case 3 — many concurrent callers crossing the threshold together start
/// EXACTLY one upgrade: the CAS state machine stays single-fire under a race.
#[tokio::test(flavor = "multi_thread")]
async fn exactly_one_upgrade_under_a_race_across_the_threshold() {
    let engine = auto_engine();
    let function = auto_function(&engine, &identity_wat());
    let stats = engine.tier_stats();
    assert_eq!(function.tier(), Tier::Fast);

    // The barrier releases all callers together, so they contend at the threshold.
    let barrier = Arc::new(Barrier::new(16));
    let callers: Vec<_> = (0..16)
        .map(|_| {
            let function = Arc::clone(&function);
            let barrier = Arc::clone(&barrier);
            let params = echo_params();
            tokio::spawn(async move {
                barrier.wait().await;
                call_once(&function, &params).await
            })
        })
        .collect();
    for caller in callers {
        caller.await.expect("caller task must not panic");
    }
    wait_for_tier(&function, Tier::Optimized).await;

    assert_eq!(
        stats.upgrades_started.load(Ordering::Relaxed),
        1,
        "the race scheduled more than one upgrade"
    );
    assert_eq!(stats.upgrades_completed.load(Ordering::Relaxed), 1);
}
