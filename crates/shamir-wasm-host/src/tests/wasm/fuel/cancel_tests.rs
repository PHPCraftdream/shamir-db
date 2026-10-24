//! Scenario D — cancellation (RV-WASM-HOST-8AFCE04E): a future dropped
//! while its active Store still holds an exclusive lease must reconcile
//! exactly via `AccountedStore::Drop` — consumed fuel charged, remainder
//! refunded, one settlement per Store, no live leases — both for a plain
//! gated import and for a parent→child→gated-db chain where only the
//! ACTIVE child Store owns fuel.
//!
//! Production mutations that break the single-Store test: (a) moving the
//! refund to trailing code after the `.await` in `WasmFunction::call` (no
//! Drop settlement) leaves the pool at 0 after cancellation (lower bound
//! fails); (b) dropping the up-front `pool_take` reservation while the
//! refund path is RETAINED pushes the free pool back to (or above) B —
//! the suspended-phase zero check and the post-drop `free < g` upper
//! bound (which rejects any free >= B, not a specific value) fail.
//!
//! Production mutation that breaks the chain test: dropping the SUSPEND
//! refund in `bindings/call.rs` leaves the parent's unused fuel
//! un-refunded while the child runs, so the exact `free == B − C`
//! conservation snapshot fails — but the SAME mutation may instead starve
//! the child and prevent the chain from ever REACHING the gate; the
//! bounded poll protocol catches that shape too (early `Poll::Ready` →
//! "call finished unexpectedly", or the "never suspended" panic), not
//! only the conservation assertion.

use super::support::{
    assert_settled, echo_child_wat, fn_from_wat, pool, CALL_GATED_CHILD_WAT, GATED_DB_CHILD_WAT,
};
use crate::gateway::db_gateway::DbGateway;
use crate::{
    FnBatch, FnCtx, FuelObserver, FunctionError, FunctionRegistry, Params, ShamirFunction,
    WasmEngine,
};
use shamir_types::types::value::QueryValue;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

const CALIB_POOL: u64 = 1_000_000_000;

/// DbGateway whose `get` signals `hit` and then pends forever — a NON-`call`
/// async host import, so the active Store keeps its exclusive lease while
/// suspended and only `AccountedStore::Drop` can reconcile.
struct GatedDb {
    hit: AtomicBool,
}

#[async_trait::async_trait]
impl DbGateway for GatedDb {
    async fn get(
        &self,
        _repo: &str,
        _table: &str,
        _key: QueryValue,
    ) -> Result<Option<QueryValue>, String> {
        self.hit.store(true, Ordering::SeqCst);
        std::future::pending::<Result<Option<QueryValue>, String>>().await
    }
    async fn insert(&self, _: &str, _: &str, _: QueryValue) -> Result<QueryValue, String> {
        Err("unused".into())
    }
    async fn query(
        &self,
        _: &str,
        _: &str,
        _: Option<QueryValue>,
    ) -> Result<Vec<QueryValue>, String> {
        Err("unused".into())
    }
    async fn execute(&self, _: &[u8]) -> Result<Vec<u8>, String> {
        Err("unused".into())
    }
}

/// Bounded poll protocol: every `Pending` is re-checked against the
/// gateway signal AND the gate-boundary fuel observation — no arbitrary
/// first-Pending assumption, no sleeps, no timing race.
fn poll_until_gated<F>(fut: &mut F, gated: &GatedDb, observer: &FuelObserver)
where
    F: Future<Output = Result<QueryValue, FunctionError>> + Unpin,
{
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    for _ in 0..100_000 {
        match Pin::new(&mut *fut).poll(&mut cx) {
            Poll::Pending => {}
            Poll::Ready(r) => panic!("call finished unexpectedly: {r:?}"),
        }
        if gated.hit.load(Ordering::SeqCst) && observer.gate_observed() > 0 {
            return;
        }
    }
    panic!("call never suspended on the gated db_get import");
}

/// RV-WASM-HOST-8AFCE04E — single Store suspended on a gated import:
/// cancelling the future charges only the consumed fuel (`G/2 < free < G`)
/// and settles exactly once.
#[tokio::test]
async fn rv_wasm_host_8afce04e_cancellation_charges_consumed_fuel_via_gated_db_import() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let wf = fn_from_wat(&engine, GATED_DB_CHILD_WAT);
    let gated = Arc::new(GatedDb {
        hit: AtomicBool::new(false),
    });
    let budget = Arc::new(AtomicU64::new(CALIB_POOL));
    let obs = FuelObserver::new();
    let ctx = FnCtx::new()
        .with_db(gated.clone(), "r".into())
        .with_fuel_budget(budget.clone())
        .with_fuel_observer(obs.clone());
    let batch = FnBatch::new();
    let params = Params::new();

    let mut fut = Box::pin(wf.call(&ctx, &batch, &params));
    poll_until_gated(&mut fut, &gated, &obs);

    // While suspended the ACTIVE Store owns the WHOLE grant (the entry
    // lease took the entire pool G and nothing has been refunded yet), so
    // the free pool must be EXACTLY 0: one outstanding lease, burn charged
    // inside the Store, zero settlements so far.
    let g = CALIB_POOL;
    assert_eq!(
        pool(&budget),
        0,
        "the whole grant must be leased out while the import is pending"
    );
    assert_eq!(obs.install_count(), 1);
    assert_eq!(obs.last_install(), g, "the entry lease is the whole pool");
    assert_eq!(
        obs.last_install_actual(),
        g,
        "reservation and installed fuel must match (no clamp)"
    );
    assert_eq!(obs.settles(), 0, "no settlement while the future is live");
    let gate_burn = obs.last_install() - obs.gate_child_fuel();
    assert!(
        gate_burn > 0,
        "the guest's burn must be charged inside the Store"
    );
    assert_eq!(
        obs.stores_with_fuel(),
        1,
        "exactly one Store (the active child) may hold executable fuel"
    );

    drop(fut); // cancel while suspended on the pending host import

    let free = pool(&budget);
    assert!(
        free > g / 2,
        "unconsumed majority must be refunded on Drop; free={free}"
    );
    assert!(free < g, "the consumed burn must stay charged; free={free}");
    assert_eq!(obs.settles(), 1, "exactly one settlement for the one Store");
    assert_settled(g, &budget, &obs);
}

/// Scenario D chain: parent WASM → child WASM → gated db. At the gate the
/// snapshot `C = (ancestor spent) + (active child spent)` is taken from
/// REAL Store fuel observation (`get_fuel` at the import boundary); the
/// ancestors' Stores hold zero executable fuel; exactly one active lease.
/// Dropping the future refunds everything except `C` (`free == B − C`),
/// every Store settles exactly once, and the SAME depleted pool then
/// rejects a calibrated probe `Q` with `B − C < Q ≤ B` while a fresh
/// `B = T_probe` pool succeeds exactly.
#[tokio::test]
async fn cancellation_chain_ancestors_zero_one_active_lease_exact_refund() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let parent = fn_from_wat(&engine, CALL_GATED_CHILD_WAT);
    let child = fn_from_wat(&engine, GATED_DB_CHILD_WAT);
    let probe = fn_from_wat(&engine, &echo_child_wat(30_000));

    let reg = Arc::new(FunctionRegistry::new());
    reg.register("parent", parent.clone()).unwrap();
    reg.register("child", child).unwrap();
    let probe_reg = super::support::registry1("probe", probe.clone());
    let empty = Params::new();
    let empty_batch = FnBatch::new();

    // Calibrate the probe's exact standalone demand T_probe.
    let pb = Arc::new(AtomicU64::new(CALIB_POOL));
    super::support::invoke(&probe_reg, "probe", Some(pb.clone()), None, &empty)
        .await
        .unwrap();
    let t_probe = CALIB_POOL - pool(&pb);
    assert!(t_probe > 3);

    // ---- Calibration pass: how much does the chain burn to REACH the gate?
    let gated0 = Arc::new(GatedDb {
        hit: AtomicBool::new(false),
    });
    let obs0 = FuelObserver::new();
    let budget0 = Arc::new(AtomicU64::new(CALIB_POOL));
    let ctx0 = FnCtx::new()
        .with_registry(reg.clone())
        .with_db(gated0.clone(), "r".into())
        .with_fuel_budget(budget0.clone())
        .with_fuel_observer(obs0.clone());
    let mut fut0 = Box::pin(parent.call(&ctx0, &empty_batch, &empty));
    poll_until_gated(&mut fut0, &gated0, &obs0);
    drop(fut0);
    let c = obs0.burned(); // total burned up to the gate, both Stores
    assert!(c > 0 && c < CALIB_POOL);
    assert_eq!(obs0.settles(), 2);

    // Budget sized so the DEPLETED pool itself (never refilled) is exactly
    // one probe unit short: B = C + T_probe − 1 leaves free = B − C =
    // T_probe − 1 after the drop — strictly below the probe's measured
    // demand T_probe, while a FRESH pool of B still funds it:
    // B − C < T_probe ≤ B.
    let b = c + t_probe - 1;
    assert!(b > c, "budget must still reach the gate with slack");
    assert!(
        b >= t_probe,
        "a fresh pool of B must be able to fund the probe"
    );

    // ---- Measured pass with the tight-but-sufficient budget.
    let gated = Arc::new(GatedDb {
        hit: AtomicBool::new(false),
    });
    let obs = FuelObserver::new();
    let budget = Arc::new(AtomicU64::new(b));
    let ctx = FnCtx::new()
        .with_registry(reg)
        .with_db(gated.clone(), "r".into())
        .with_fuel_budget(budget.clone())
        .with_fuel_observer(obs.clone());
    let mut fut = Box::pin(parent.call(&ctx, &empty_batch, &empty));
    poll_until_gated(&mut fut, &gated, &obs);

    // The gate-boundary snapshot: real child Store fuel observed at the
    // import boundary; the parent's segment spend from its real
    // install/suspend records; the child's spend = child grant − observed.
    let parent_grant = b; // the initial lease is the whole budget
    let parent_spent = parent_grant - obs.suspended_fuel_last();
    let child_grant = obs.last_install(); // last install before the gate is the child's
    let child_spent = child_grant - obs.gate_child_fuel();
    let c2 = parent_spent + child_spent;
    assert_eq!(c2, c, "gate demand is deterministic across passes");
    assert_eq!(
        obs.suspends(),
        1,
        "the ancestor Store is suspended exactly once"
    );
    assert_eq!(obs.gate_child_fuel(), child_grant - child_spent);
    // The ANCESTOR's revoke was observed at REAL fuel exactly 0 (not merely
    // counted), and the active child is the ONLY Store holding fuel.
    assert_eq!(
        obs.stores_with_fuel(),
        1,
        "only the active child Store may hold executable fuel at the gate"
    );
    // Exactly one ACTIVE lease: free == B − parent_spent − child_grant.
    assert_eq!(
        pool(&budget),
        b - parent_spent - child_grant,
        "exactly one active lease (the child's) may be outstanding"
    );

    // Drop the future WITHOUT the gateway ever releasing.
    drop(fut);
    // free == B − C exactly: the child refunds everything it did not burn,
    // the suspended parent refunds zero, no lease survives.
    assert_eq!(
        pool(&budget),
        b - c,
        "drop must refund all but the consumed C"
    );
    assert_eq!(obs.settles(), 2, "settlement exactly once per Store");
    assert_settled(b, &budget, &obs);

    // Reuse the SAME DEPLETED pool — NEVER refilled — for the calibrated
    // probe: its free remainder is exactly T_probe − 1, one unit below the
    // probe's measured demand → typed fuel failure.
    let free = pool(&budget);
    assert_eq!(free, b - c, "depleted remainder after the drop");
    assert!(free < t_probe && t_probe <= b, "B − C < T_probe ≤ B");
    let err = super::support::invoke(&probe_reg, "probe", Some(budget.clone()), None, &empty)
        .await
        .expect_err("probe must exceed the depleted pool by exactly one unit");
    assert!(matches!(err, FunctionError::FuelExhausted), "got: {err:?}");
    // The failing probe consumed its ENTIRE remaining lease — wasmtime
    // saturates the Store's fuel at 0 on OutOfFuel and the lease's
    // settlement refunds that 0 — so the pool must be EXACTLY 0.
    assert_eq!(
        pool(&budget),
        0,
        "a fuel-failed probe burns its whole lease down to zero"
    );

    // Fresh pool of the SAME size B (≥ T_probe): exact expected success and
    // exact expected free = B − T_probe.
    let fresh = Arc::new(AtomicU64::new(b));
    let result = super::support::invoke(&probe_reg, "probe", Some(fresh.clone()), None, &empty)
        .await
        .unwrap_or_else(|e| panic!("fresh adequate pool must succeed: {e:?}"));
    assert_eq!(result, QueryValue::Map(Params::new().raw().clone()));
    assert_eq!(
        pool(&fresh),
        b - t_probe,
        "exact expected free after success"
    );
}
