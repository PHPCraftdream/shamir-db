//! Fuel-lease budget scenarios A–C plus a depth-3 chain and the exactness
//! cards RV-WASM-HOST-6F709AC2 and RV-WASM-HOST-A6416C6A.
//!
//! Calibration is NOT circular: the exact total demand `T` is the sum of
//! real Store fuel SEGMENT consumption — each lease's grant minus its
//! refunds, measured by the per-invocation `FuelObserver` against real
//! `set_fuel`/`get_fuel` state — and the child demand `K` is measured in a
//! fully independent standalone run. `B = T − 1` then fails and `B = T`
//! succeeds exactly, with `assert_settled` proving no live leases.

use super::support::{
    assert_fuel_typed, assert_settled, d_params, echo_child_wat, echo_result, fn_from_wat, invoke,
    pool, registry1, relay_wat, THREE_CALL_PARENT_WAT,
};
use crate::{FuelObserver, Params, WasmEngine};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

const CALIB_POOL: u64 = 1_000_000_000;

fn budget(v: u64) -> Arc<AtomicU64> {
    Arc::new(AtomicU64::new(v))
}

fn chain_registry(
    parent: Arc<crate::WasmFunction>,
    middle: Arc<crate::WasmFunction>,
    child: Arc<crate::WasmFunction>,
) -> Arc<crate::FunctionRegistry> {
    let reg = Arc::new(crate::FunctionRegistry::new());
    reg.register("parent", parent).unwrap();
    reg.register("middle", middle).unwrap();
    reg.register("child", child).unwrap();
    reg
}

/// Independent standalone burn of a single-Store fixture (huge pool).
async fn standalone_cost(reg: &Arc<crate::FunctionRegistry>, name: &str, params: &Params) -> u64 {
    let b = budget(CALIB_POOL);
    invoke(reg, name, Some(b.clone()), None, params)
        .await
        .unwrap_or_else(|e| panic!("{name} calibration must succeed: {e:?}"));
    let used = CALIB_POOL - pool(&b);
    assert!(used > 0, "{name} must burn fuel");
    used
}

/// Scenario A (RV-WASM-HOST-6F709AC2): finite parent-child with work
/// before AND after the call; independently measured parent `P` and child
/// `K`; `max(P, K) < T = P + K`.
///
/// Production mutations that break this test (each STATED SEPARATELY —
/// they are distinct, incompatible-in-effect reverts, not one mutation):
/// * **Reservation retained, refund removed** (the suspend path keeps the
///   parent's unused capacity debited): the chain only STARVES — the
///   child draws less at `B = T` (Ok case fails) and there is NO
///   duplication effect (the capacity is simply lost).
/// * **Executable parent fuel left installed while ALSO refunding** its
///   unused remainder: the aggregate capacity is duplicated, but the
///   parent's reacquire `set_fuel` OVERWRITES the leftover, which can
///   hide it behaviorally — the check that catches this mutation is the
///   actual-zero observation (`check_settled`'s "suspend must revoke to
///   0" invariant), not the budget arithmetic.
/// Mutating only the entry `pool_take` away does NOT invalidate this
/// test's calibration: `T` comes from the independent per-segment
/// instrumentation (real installs/refunds), not from `B − F` of the run
/// under test.
#[tokio::test]
async fn rv_wasm_host_6f709ac2_parent_child_work_both_sides_exact_t_budgets() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let child = fn_from_wat(&engine, &echo_child_wat(4_000));
    let parent = fn_from_wat(&engine, &relay_wat(2_000, 6_000, "child"));

    // Independent child demand K.
    let k = standalone_cost(&registry1("child", child.clone()), "child", &d_params(0)).await;

    // Chain run with per-segment instrumentation: T = real total burned,
    // P = T − K (the parent's own segments, incl. the host-call cost).
    let obs = FuelObserver::new();
    let t_budget = budget(CALIB_POOL);
    let chain_reg = chain_registry(parent.clone(), child.clone(), child.clone());
    let params = d_params(1);
    invoke(
        &chain_reg,
        "parent",
        Some(t_budget.clone()),
        Some(obs.clone()),
        &params,
    )
    .await
    .unwrap_or_else(|e| panic!("huge-budget chain must succeed: {e:?}"));
    let t = obs.burned();
    let p = t - k;
    assert!(p > 0, "parent segments must burn fuel; t={t}, k={k}");
    assert!(
        p.max(k) < t,
        "each side strictly below the total: p={p}, k={k}, t={t}"
    );

    // Per-Store reset control (explicit test-only harness: each Store runs
    // STANDALONE with its own pool — the production subject is not mocked):
    // each side's own demand fits under B = T − 1, so the chain failure
    // below is genuinely about the AGGREGATE. The solo parent's burn is
    // slightly BELOW the chain parent's segments `p` (the chain charges the
    // host-call instruction too) — the control only needs the solo demand
    // to fit under B.
    let b = t - 1;
    invoke(
        &registry1("child", child.clone()),
        "child",
        Some(budget(b)),
        None,
        &d_params(0),
    )
    .await
    .unwrap_or_else(|e| panic!("standalone child must fit B=T-1 (k={k}): {e:?}"));
    let solo_obs = FuelObserver::new();
    invoke(
        &registry1("parent", parent.clone()),
        "parent",
        Some(budget(b)),
        Some(solo_obs.clone()),
        &d_params(0), // d = 0: no child call
    )
    .await
    .unwrap_or_else(|e| panic!("standalone parent must fit B=T-1 (p={p}): {e:?}"));
    assert!(
        solo_obs.burned() <= p,
        "solo parent burn must not exceed its chain segments"
    );

    // B = T: exact success, exact expected value, pool drained to 0.
    let obs_ok = FuelObserver::new();
    let ok_budget = budget(t);
    let result = invoke(
        &chain_reg,
        "parent",
        Some(ok_budget.clone()),
        Some(obs_ok.clone()),
        &params,
    )
    .await
    .unwrap_or_else(|e| panic!("exact-budget chain (T={t}) must succeed: {e:?}"));
    assert_eq!(result, echo_result(&params), "identity echo expected");
    assert_eq!(pool(&ok_budget), 0, "exact pool must be fully consumed");
    assert_eq!(obs_ok.burned(), t, "burn must equal the calibrated total");
    assert_eq!(
        obs_ok.settles(),
        2,
        "one settlement per Store, no live leases"
    );
    assert_settled(t, &ok_budget, &obs_ok);

    // B = T − 1: typed per-Store fuel exhaustion in the parent's post-call
    // segment; the failing Store's remaining fuel is exactly 0 (wasmtime
    // saturates), so the pool reconciles to exactly 0.
    let obs_short = FuelObserver::new();
    let short_budget = budget(t - 1);
    let err = invoke(
        &chain_reg,
        "parent",
        Some(short_budget.clone()),
        Some(obs_short.clone()),
        &params,
    )
    .await
    .expect_err("chain one unit short of T must fail");
    assert_fuel_typed(err);
    assert!(obs_short.burned() < t);
    assert_eq!(
        pool(&short_budget),
        0,
        "child consumed its short lease; nothing may remain live"
    );
    assert_settled(t - 1, &short_budget, &obs_short);
}

/// Depth-3 exact variant: parent → middle → child, THREE real Stores, each
/// with work before and after its own relay call; independently measured
/// per-Store demands P + M + K = T; `B = T` succeeds with pool 0 and three
/// settlements, `B = T − 1` fails typed.
///
/// Two DISTINCT revert shapes, detected differently:
/// * omitting the per-suspend REFUND while the up-front reservation is
///   retained: the deeper level draws less and the `B = T` case starves
///   (fails);
/// * omitting the per-suspend REVOKE while the refund is retained: the
///   budget arithmetic need not change — detection is the observer's
///   actual-zero check (`check_settled`'s "suspend must revoke to REAL
///   fuel 0" invariant), not starvation.
/// NO detection is claimed in THIS test's fixtures for dropping the
/// `min(parent_unused, …)` reacquisition bound.
#[tokio::test]
async fn rv_wasm_host_6f709ac2_depth3_three_stores_exact_budgets() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let child = fn_from_wat(&engine, &echo_child_wat(2_000));
    let middle = fn_from_wat(&engine, &relay_wat(1_500, 2_500, "child"));
    let parent = fn_from_wat(&engine, &relay_wat(1_000, 3_000, "middle"));

    let k = standalone_cost(&registry1("child", child.clone()), "child", &d_params(0)).await;
    let m = standalone_cost(&registry1("middle", middle.clone()), "middle", &d_params(0)).await;

    let obs = FuelObserver::new();
    let chain_reg = chain_registry(parent, middle, child);
    let params = d_params(2); // two relay hops: parent → middle → child
    invoke(
        &chain_reg,
        "parent",
        Some(budget(CALIB_POOL)),
        Some(obs.clone()),
        &params,
    )
    .await
    .unwrap_or_else(|e| panic!("huge-budget depth-3 chain must succeed: {e:?}"));
    let t = obs.burned();
    let p = t - m - k;
    assert!(
        p > 0 && m > 0,
        "every Store must burn: p={p}, m={m}, k={k}, t={t}"
    );
    assert_eq!(obs.settles(), 3, "three Stores, three settlements");
    assert_eq!(obs.suspends(), 2, "parent and middle each suspended once");

    // B = T: exact success, pool 0, every Store settled once.
    let obs_ok = FuelObserver::new();
    let ok_budget = budget(t);
    let result = invoke(
        &chain_reg,
        "parent",
        Some(ok_budget.clone()),
        Some(obs_ok.clone()),
        &params,
    )
    .await
    .unwrap_or_else(|e| panic!("exact depth-3 chain (T={t}) must succeed: {e:?}"));
    assert_eq!(result, echo_result(&params));
    assert_eq!(pool(&ok_budget), 0);
    assert_eq!(obs_ok.burned(), t);
    assert_eq!(obs_ok.settles(), 3);
    assert_settled(t, &ok_budget, &obs_ok);

    // B = T − 1: typed fuel failure, pool reconciled to 0.
    let obs_short = FuelObserver::new();
    let short_budget = budget(t - 1);
    let err = invoke(
        &chain_reg,
        "parent",
        Some(short_budget.clone()),
        Some(obs_short.clone()),
        &params,
    )
    .await
    .expect_err("depth-3 chain one unit short of T must fail");
    assert_fuel_typed(err);
    assert_eq!(pool(&short_budget), 0);
    assert_settled(t - 1, &short_budget, &obs_short);
}

/// Scenario A companion (RV-WASM-HOST-A6416C6A): aggregate (exclusive-pool)
/// exhaustion is distinguishable from per-Store fuel exhaustion — with the
/// SAME fixtures, every single Store fits a pool that the 2-Store chain
/// cannot.
///
/// Oracle note: the A6416C6A card is itself a TEST repair — there is no
/// separate independent production fix "behind" it. Its sharper form (typed
/// matching, per-Store control, instrumented exact calibration) guards the
/// same production exclusive-lease mechanism. The production mutation that
/// breaks this test: granting each Store fuel WITHOUT removing it from the
/// shared pool (per-Store reset) — the chain at `pool = T − 1` then
/// succeeds.
#[tokio::test]
async fn rv_wasm_host_a6416c6a_aggregate_exhaustion_typed_and_per_store_control() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let child = fn_from_wat(&engine, &echo_child_wat(4_000));
    let parent = fn_from_wat(&engine, &relay_wat(2_000, 6_000, "child"));
    let chain_reg = chain_registry(parent.clone(), child.clone(), child);
    let params = d_params(1);

    let obs = FuelObserver::new();
    let t_budget = budget(CALIB_POOL);
    invoke(
        &chain_reg,
        "parent",
        Some(t_budget.clone()),
        Some(obs.clone()),
        &params,
    )
    .await
    .unwrap();
    let t = obs.burned();
    let b = t - 1;

    // Per-Store control: a single Store (the chain's own parent fixture
    // with d = 0, so its demand is a strict subset of the chain's) fits B.
    let solo_obs = FuelObserver::new();
    invoke(
        &registry1("parent", parent),
        "parent",
        Some(budget(b)),
        Some(solo_obs),
        &d_params(0),
    )
    .await
    .unwrap_or_else(|e| panic!("per-Store control must succeed at B=T-1: {e:?}"));

    // The AGGREGATE is one unit short: typed fuel failure.
    let err = invoke(&chain_reg, "parent", Some(budget(b)), None, &params)
        .await
        .expect_err("aggregate-short chain must fail");
    assert_fuel_typed(err);
}

/// Scenario B: child small, parent post-call work substantial. The
/// checkpoint proves the child COMPLETED and the parent's reacquisition
/// equals its new reservation — `min(own_cap_left, amount just
/// relinquished)`, pool-limited at `B = T` to exactly the post-call
/// demand; with one unit short the PARENT traps during its post-call
/// segment (the child having completed).
///
/// Production mutation that breaks this test: reacquiring in
/// `bindings/call.rs` WITHOUT first revoking the parent's executable fuel
/// (leaving parent fuel live during the transfer) is caught by the
/// observer's actual-zero observation — `check_settled`'s "suspend must
/// revoke to REAL fuel 0" invariant — not by budget arithmetic.
/// DETECTION IS NOT CLAIMED for the remaining reacquire-amount variant
/// (reacquiring `own_cap_left` only, i.e. dropping the
/// `min(parent_unused, …)` limit) in THIS test's fixtures: no `B = T`
/// Ok/Err outcome, conservation snapshot or observer invariant here is
/// asserted to change under it.
#[tokio::test]
async fn parent_post_call_work_needs_exact_reacquisition() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let child = fn_from_wat(&engine, &echo_child_wat(2_000));
    // small pre, BIG post
    let parent = fn_from_wat(&engine, &relay_wat(1_000, 60_000, "child"));
    let chain_reg = chain_registry(parent, child.clone(), child);
    let params = d_params(1);

    let k = standalone_cost(
        &registry1("child", fn_from_wat(&engine, &echo_child_wat(2_000))),
        "child",
        &d_params(0),
    )
    .await;

    let obs = FuelObserver::new();
    let t_budget = budget(CALIB_POOL);
    invoke(
        &chain_reg,
        "parent",
        Some(t_budget.clone()),
        Some(obs.clone()),
        &params,
    )
    .await
    .unwrap();
    let t = obs.burned();

    // Adequate budget: success; reacquisition checkpoint.
    let obs_ok = FuelObserver::new();
    let ok_budget = budget(t);
    let result = invoke(
        &chain_reg,
        "parent",
        Some(ok_budget.clone()),
        Some(obs_ok.clone()),
        &params,
    )
    .await
    .unwrap_or_else(|e| panic!("adequate chain must succeed: {e:?}"));
    assert_eq!(result, echo_result(&params));
    // Child completed AND settled (checkpoint), exactly one suspend, and
    // the parent reacquired exactly once afterwards.
    assert_eq!(
        obs_ok.install_count(),
        3,
        "parent entry + child + parent reacquire"
    );
    assert_eq!(obs_ok.suspends(), 1);
    assert_eq!(
        obs_ok.settles(),
        2,
        "child completed and settled; parent settled once"
    );
    // The parent's pre-call segment: grant − suspended refund. At B = T the
    // parent's initial grant is the whole pool, and the reacquisition is
    // limited by the pool to EXACTLY the post-call demand:
    // reacquire == T − pre − K == suspended_fuel_last − K. Both getters are
    // CHRONOLOGICAL (max event sequence): the last install here is the
    // parent's REACQUIRE, not the child's earlier entry install — the
    // pre-repair reverse-creation-order getter would have returned the
    // child's grant and this assertion would fail.
    let pre = t - obs_ok.suspended_fuel_last();
    let reacquired = obs_ok.last_install();
    assert_eq!(
        reacquired,
        t - pre - k,
        "reacquired fuel must equal the new (pool-limited) reservation: \
         t={t}, pre={pre}, k={k}"
    );
    assert!(
        reacquired > 0,
        "parent must have reacquired executable fuel"
    );
    // Scenario B checkpoint (chronological-getter repair): the last
    // install is the parent's NEW lease, and its ACTUALLY-installed fuel
    // equals the reservation (no clamp/cast in the reacquire install).
    assert_eq!(
        obs_ok.last_install_actual(),
        reacquired,
        "observed installed fuel must equal the reacquire reservation"
    );
    assert_settled(t, &ok_budget, &obs_ok);

    // One unit short: the parent's post-call segment runs out — typed fuel
    // trap DURING post-call work (child still completed and settled).
    let obs_short = FuelObserver::new();
    let short_budget = budget(t - 1);
    let err = invoke(
        &chain_reg,
        "parent",
        Some(short_budget.clone()),
        Some(obs_short.clone()),
        &params,
    )
    .await
    .expect_err("insufficient total must trap the parent's post-call work");
    assert_fuel_typed(err);
    assert_eq!(
        obs_short.settles(),
        2,
        "child completed and both Stores settled"
    );
    assert_eq!(
        obs_short.suspends(),
        1,
        "parent was suspended in the host call"
    );
    assert_settled(t - 1, &short_budget, &obs_short);
}

/// Scenario C: ONE parent makes THREE sequential child calls with
/// substantial work between and after; the independent segment-total `T`
/// drains to an exact final pool `B − T`, zero live leases; a tight budget
/// above every individual Store demand but below the total fails typed.
///
/// Production mutation that breaks this test: dropping the per-suspend
/// baseline reset (the parent's `fuel_baseline` not zeroed at suspend, or
/// not restored at reacquire) double-counts or loses parent segments — the
/// `B = T` case then leaves a nonzero final pool or mischarges a segment
/// (the recomputation assertion and the observer's per-Store burned /
/// own_cap-evolution assertions fail). That mutation is ALSO caught
/// structurally and independently: the observer compares the production
/// `baseline` argument against its OWN per-Store baseline at every
/// suspend/settle and `check_settled` fails — the calibration expectation
/// here does not change under it. The instrumentation mutation that breaks
/// the per-Store reads is reverting the observer's independent
/// calibration (computing burned from the production baseline with
/// saturating subtraction again).
#[tokio::test]
async fn rv_wasm_host_6f709ac2_three_sequential_children_exact_pool_and_reacquire_ceiling() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let child = fn_from_wat(&engine, &echo_child_wat(2_500));
    let parent = fn_from_wat(&engine, THREE_CALL_PARENT_WAT);
    let chain_reg = chain_registry(parent, child.clone(), child);
    let empty = Params::new();

    // Independent child demand K (each of the three calls burns exactly K).
    let k = standalone_cost(
        &registry1("child", fn_from_wat(&engine, &echo_child_wat(2_500))),
        "child",
        &empty,
    )
    .await;

    let obs = FuelObserver::new();
    let t_budget = budget(CALIB_POOL);
    invoke(
        &chain_reg,
        "parent",
        Some(t_budget.clone()),
        Some(obs.clone()),
        &empty,
    )
    .await
    .unwrap();
    let t = obs.burned();
    let p = t - 3 * k; // the parent's own four segments (incl. call costs)
    assert!(p > 0, "parent segments must burn; t={t}, k={k}");
    assert_eq!(obs.suspends(), 3, "three nested host calls");
    assert_eq!(
        obs.settles(),
        4,
        "one settlement per Store under the huge pool"
    );

    // Per-Store ceiling control: one child alone fits the tight pool below.
    let b = t - 1;
    assert!(k <= b, "individual Store demand must fit the tight budget");

    // B = T: exact final pool, zero live leases.
    let obs_ok = FuelObserver::new();
    let ok_budget = budget(t);
    invoke(
        &chain_reg,
        "parent",
        Some(ok_budget.clone()),
        Some(obs_ok.clone()),
        &empty,
    )
    .await
    .unwrap_or_else(|e| panic!("exact three-call chain must succeed: {e:?}"));
    assert_eq!(pool(&ok_budget), 0, "final pool must be exactly B − T");
    assert_eq!(obs_ok.burned(), t);
    assert_eq!(
        obs_ok.settles(),
        4,
        "zero live leases: every Store settled once"
    );
    assert_eq!(obs_ok.suspends(), 3);
    // Reacquire ceiling: across all three reacquisitions the parent never
    // charged more than its own remaining cap — segments recompute exactly.
    assert_eq!(
        3 * k + p,
        t,
        "three child segments plus parent segments == T"
    );
    // Per-Store INDEPENDENT recorded demands (observer's own calibration,
    // not derived from B − F): the parent's recorded own segments equal the
    // derived P, and each of the three child Stores recorded exactly K.
    let parent_id = 0; // the first Store entered
    assert_eq!(
        obs_ok.store_burned(parent_id),
        Some(p),
        "parent's recorded own segments must equal P (independent of the \
         t − 3k derivation)"
    );
    for child_store in 1..=3 {
        assert_eq!(
            obs_ok.store_burned(child_store),
            Some(k),
            "child store #{child_store} must record exactly K"
        );
    }
    // The parent's OWN cap evolves by the ACTUAL segment deltas — read
    // from the observer's own baseline (entry install, suspend deltas,
    // reacquire installs), independent of production `own_cap_left`
    // bookkeeping: cap[i+1] == cap[i] − (reserve[i] − before[i]).
    let caps = obs_ok.store_install_own_caps(parent_id);
    let reserves = obs_ok.store_install_reserves(parent_id);
    let befores = obs_ok.store_suspend_befores(parent_id);
    assert_eq!(caps.len(), 4, "entry install + three reacquires");
    assert_eq!(befores.len(), 3, "three suspensions");
    let mut expected_cap = caps[0];
    for i in 0..3 {
        let baseline_i = reserves[i]; // install preceding suspend i
        let delta = baseline_i - befores[i]; // actual segment i consumption
        expected_cap -= delta;
        assert_eq!(
            caps[i + 1],
            expected_cap,
            "own_cap after segment {i} must equal cap minus the ACTUAL delta"
        );
    }
    assert_settled(t, &ok_budget, &obs_ok);

    // Tight: above each individual demand, below the total → typed fuel Err.
    // The single-unit shortfall propagates to the LAST segment (the
    // parent's post-work), so all four Stores are still created and settle
    // exactly once each.
    let obs_tight = FuelObserver::new();
    let tight_budget = budget(b);
    let err = invoke(
        &chain_reg,
        "parent",
        Some(tight_budget.clone()),
        Some(obs_tight.clone()),
        &empty,
    )
    .await
    .expect_err("tight aggregate must fail (not depth/epoch)");
    assert_fuel_typed(err);
    assert_eq!(
        obs_tight.settles(),
        4,
        "every Store settled exactly once on the error path"
    );
    assert_settled(b, &tight_budget, &obs_tight);
}

/// FuelLease RAII discipline, exercised through the test-only
/// `lease_raii_probe` helper defined inside `fuel.rs` (the token type is
/// `pub(super)`; production types are not widened for tests): drop
/// refunds the whole reservation, `consume` transfers ownership without
/// refunding.
///
/// Production mutation that breaks this test: making `Drop` skip the
/// refund (or `consume` still refunding) breaks lease ownership — the
/// WHOLE probe test then fails (its four assertions stand and fall
/// together: `after_drop` would stay 40 instead of 100, or `after_consume`
/// would bounce back to 100). Not every mutation is caught by every
/// individual assertion — the test as a whole is the detector.
/// The production `set_fuel`-failure path itself cannot be triggered
/// without a fuel-disabled engine; its code (`Err` branch drops the lease
/// before propagating) is read-verified against this contract.
#[test]
fn fuel_lease_raii_refunds_on_drop_and_defuses_on_consume() {
    let (after_take, after_drop, after_consume, consumed) =
        crate::FuelObserver::lease_raii_probe(100, 60);
    assert_eq!(consumed, 60, "the reservation takes the requested amount");
    assert_eq!(after_take, 40, "reservation leaves the pool up front");
    assert_eq!(after_drop, 100, "an unconsumed lease is refunded on drop");
    assert_eq!(
        after_consume, 40,
        "a consumed lease stays debited after its defused Drop"
    );
}

/// RV-WASM-HOST-6F709AC2 — observer chronological-getter regression
/// (INSTRUMENTATION card, helper-level): parent installation 10, child 6,
/// parent reacquire 4 ⇒ `last_install` == 4; parent/child suspends in the
/// same inverted shape ⇒ `suspended_fuel_last` reports the chronologically
/// LATEST suspension (the parent's second), not the later-created child's.
///
/// Mutation that breaks this test: reverting the getters in `wasm/fuel.rs`
/// to reverse-Store-creation order (`.iter().rev().find_map(..last())`)
/// inverts the recorded order the assertions below pin down. This card is
/// a TEST-observer repair: there is NO separate production mutation behind
/// it — production call/settle paths are unchanged — and its role is a
/// test oracle (it is what makes the scenario-B reacquire checkpoint
/// meaningful). No claim is made about HOW the ordering is implemented
/// (lock choice or otherwise): the probe exercises only the reported
/// order of recorded events.
#[test]
fn rv_wasm_host_6f709ac2_observer_getters_report_chronological_last_events() {
    let (last, last_actual, last_suspend_before) = crate::FuelObserver::chronological_probe();
    assert_eq!(
        last, 4,
        "last install must be the parent's reacquire (4), not the child's (6)"
    );
    assert_eq!(
        last_actual, 4,
        "last_install_actual must come from the SAME chronological record"
    );
    assert_eq!(
        last_suspend_before, 2,
        "last suspend must be the parent's second (before = 2), not the child's (1)"
    );
}

/// RV-WASM-HOST-6F709AC2 — per-Store OWN cap is independent of the global
/// pool, verified with the observer's OWN calibration (not the production
/// `fuel_baseline`): a GENEROUS global pool (1e9) with a parent whose own
/// `WasmLimits::fuel` ceiling is calibrated to exactly its measured own
/// segment demand P. The exact-P parent succeeds (all three sequential
/// child calls complete; each child keeps a generous 1e9 own cap), and the
/// P − 1 parent fails TYPED only after all three children have completed
/// and settled (each store ceiling measured includes the guest allocator,
/// input/result writes and post-call work — the recorded burned segments
/// span them all).
///
/// Production mutations and EXACTLY how they are detected (the two
/// defenses OVERLAP here; the behavioral one is NOT guaranteed under
/// either mutation, because the reacquire is ALSO bounded by
/// `min(parent_unused)` — the amount just relinquished — which can mask
/// an own-cap mutation behaviorally and still fail/run correctly):
/// * skipping the `own_cap_left` decrement, or resetting it to the
///   original cap on reacquire (refill): the P − 1 Err case may or may
///   not survive the `min` bound — this test does NOT claim behavioral
///   detection. The STRUCTURAL detector is the observer's recorded
///   own_cap evolution: `check_settled` verifies each install's own_cap
///   equals the previous own_cap minus the ACTUAL segment delta, so the
///   mutation always fails settlement invariants (the same formula the
///   scenario-C card asserts explicitly).
///
/// A missed production baseline reset at SUSPEND (not zeroing
/// `fuel_baseline` before the child runs) is caught structurally: the
/// observer compares the production `baseline` argument against its own
/// per-Store baseline and `check_settled` fails. A missed reset at SETTLE
/// is NOT detected here (the already-captured local baseline is what gets
/// recorded), so no claim is made for it.
#[tokio::test]
async fn rv_wasm_host_6f709ac2_parent_own_cap_exact_p_succeeds_p_minus_1_fails_typed_after_children(
) {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let child = fn_from_wat(&engine, &echo_child_wat(2_500)); // generous own cap
    let parent_generous = fn_from_wat(&engine, THREE_CALL_PARENT_WAT);
    let chain_reg = chain_registry(parent_generous, child.clone(), child.clone());
    let empty = Params::new();

    // Calibrate: the parent's OWN total segment demand P (its recorded
    // burned segments — allocator, input/result, all post-work included)
    // and the child demand K, both from independent runs under a huge pool.
    let k = standalone_cost(
        &registry1("child", fn_from_wat(&engine, &echo_child_wat(2_500))),
        "child",
        &empty,
    )
    .await;
    let obs_cal = FuelObserver::new();
    invoke(
        &chain_reg,
        "parent",
        Some(budget(CALIB_POOL)),
        Some(obs_cal.clone()),
        &empty,
    )
    .await
    .unwrap();
    let p = obs_cal
        .store_burned(0)
        .expect("parent Store must be recorded");
    assert_eq!(obs_cal.settles(), 4);
    assert!(p > 0, "parent must burn its own segments");
    // Structural settlement + own_cap-evolution invariants on the
    // calibration run too (check_settled verifies each recorded own_cap
    // follows prior-actual − before segment deltas).
    obs_cal
        .check_settled()
        .unwrap_or_else(|e| panic!("calibration run invariants broken: {e}"));

    // Exact own-cap parent: succeeds; its recorded burned equals P exactly
    // (consumption is deterministic and the grant exactly covers it).
    let parent_exact = Arc::new(
        crate::WasmFunction::from_wat(
            engine.clone(),
            THREE_CALL_PARENT_WAT,
            crate::WasmLimits {
                fuel: p,
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );
    let reg_exact = chain_registry(parent_exact, child.clone(), child.clone());
    let obs_ok = FuelObserver::new();
    let ok_budget = budget(CALIB_POOL);
    let result = invoke(
        &reg_exact,
        "parent",
        Some(ok_budget.clone()),
        Some(obs_ok.clone()),
        &empty,
    )
    .await
    .unwrap_or_else(|e| panic!("parent with exact own cap P={p} must succeed: {e:?}"));
    assert_eq!(result, echo_result(&empty), "identity echo expected");
    assert_eq!(
        obs_ok.store_burned(0),
        Some(p),
        "the exact-P parent records exactly its calibrated own demand"
    );
    assert_eq!(obs_ok.settles(), 4, "all three children + parent settled");
    for child_store in 1..=3 {
        assert_eq!(
            obs_ok.store_burned(child_store),
            Some(k),
            "each generous child records exactly its independent demand K"
        );
    }
    assert_settled(CALIB_POOL, &ok_budget, &obs_ok);

    // One unit below the parent's own cap: the chain still reaches and
    // completes all THREE child calls (each child is generous), but the
    // parent's own ceiling starves its own segments — typed failure.
    let parent_short = Arc::new(
        crate::WasmFunction::from_wat(
            engine.clone(),
            THREE_CALL_PARENT_WAT,
            crate::WasmLimits {
                fuel: p - 1,
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );
    let reg_short = chain_registry(parent_short, child.clone(), child.clone());
    let obs_short = FuelObserver::new();
    let short_budget = budget(CALIB_POOL);
    let err = invoke(
        &reg_short,
        "parent",
        Some(short_budget.clone()),
        Some(obs_short.clone()),
        &empty,
    )
    .await
    .expect_err("parent one unit below its own cap must fail");
    assert_fuel_typed(err);
    assert_eq!(
        obs_short.settles(),
        4,
        "all three generous children completed and settled; parent settled once"
    );
    assert_eq!(
        obs_short.suspends(),
        3,
        "the parent was suspended for each of the three nested calls"
    );
    // The failing parent saturated its own grant to 0 (wasmtime OutOfFuel
    // saturates), so its recorded burned is EXACTLY P − 1.
    assert_eq!(
        obs_short.store_burned(0),
        Some(p - 1),
        "the P−1 parent burns its whole own grant"
    );
    assert_settled(CALIB_POOL, &short_budget, &obs_short);
}

/// RV-WASM-HOST-6F709AC2 — exact single-Store fuel-boundary probe
/// (SOURCE-REASONED ONLY from the wasmtime 48.0.5 sources; not executed
/// locally per policy — CI pending): the minimal passing grant for the
/// echo fixture should be EXACTLY its measured total demand
/// T — grant T succeeds and grant T − 1 fails typed, burning its whole
/// lease down to 0. This is only enforceable because the fixtures END in a
/// marker function call (see `support::HEADER`): wasmtime (48.0.5) checks
/// fuel ONLY at function entries and loop back-edges, with the trap fired
/// when a check observes consumed >= grant — so a final check point after
/// all burn work, with only the marker's single charged `local.get`
/// following it, is EXPECTED to make the T / T − 1 straddle a real check
/// instead of unchecked-tail luck (the previous fixtures left the whole
/// echo tail unchecked). Be honest: the exact size of the charged
/// unchecked tail is wasmtime-version-dependent; if it ever exceeds one
/// unit this card fails in CI and must be recalibrated.
#[tokio::test]
async fn rv_wasm_host_6f709ac2_single_store_exact_boundary_t_passes_t_minus_1_fails() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let child = fn_from_wat(&engine, &echo_child_wat(2_000));
    let reg = registry1("child", child.clone());
    let params = d_params(0);
    let t = standalone_cost(&reg, "child", &params).await;

    // Grant exactly T: success, exact expected value.
    let result = invoke(&reg, "child", Some(budget(t)), None, &params)
        .await
        .unwrap_or_else(|e| panic!("grant exactly T={t} must succeed: {e:?}"));
    assert_eq!(result, echo_result(&params), "identity echo expected");

    // Grant T − 1: typed per-Store fuel exhaustion, whole lease consumed.
    let short = budget(t - 1);
    let err = invoke(&reg, "child", Some(short.clone()), None, &params)
        .await
        .expect_err("grant T − 1 must trap at the final boundary check");
    assert_fuel_typed(err);
    assert_eq!(
        pool(&short),
        0,
        "the failing Store saturates to 0; nothing may remain live"
    );
}
