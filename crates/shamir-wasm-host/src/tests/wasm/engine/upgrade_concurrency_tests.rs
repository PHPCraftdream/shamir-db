//! Upgrade interaction with in-flight calls and dropped functions: a call
//! that loaded the fast slot before the swap finishes correctly on it, and
//! a function dropped while its upgrade waits behind a held permit installs
//! nothing and records no failure.

use super::support::{
    auto_engine, call_once, echo_params, engine_with, identity_wat, wait_for_tier, wait_until,
    HOT_CALLS,
};
use crate::gateway::db_gateway::{DbGateway, DbGatewayError};
use crate::tests::wasm::fuel::support::GATED_DB_CHILD_WAT;
use crate::{FnBatch, FnCtx, Params, ShamirFunction, Tier, TierMode, WasmFunction, WasmLimits};
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::timeout;

/// Db gateway that gates exactly ONE `get`: it signals `entered`, pends
/// until `release` fires, then answers `Int(7)`; every later `get` returns
/// immediately so the threshold-crossing calls are not held.
struct GatedOnceDb {
    gated: AtomicBool,
    entered: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl DbGateway for GatedOnceDb {
    async fn get(
        &self,
        _repo: &str,
        _table: &str,
        _key: QueryValue,
    ) -> Result<Option<QueryValue>, DbGatewayError> {
        if self.gated.swap(true, Ordering::SeqCst) {
            return Ok(None);
        }
        self.entered.notify_one();
        self.release.notified().await;
        Ok(Some(QueryValue::Int(7)))
    }
    async fn insert(&self, _: &str, _: &str, _: QueryValue) -> Result<QueryValue, DbGatewayError> {
        Err(DbGatewayError::Execution("unused".into()))
    }
    async fn query(
        &self,
        _: &str,
        _: &str,
        _: Option<QueryValue>,
    ) -> Result<Vec<QueryValue>, DbGatewayError> {
        Err(DbGatewayError::Execution("unused".into()))
    }
    async fn execute(&self, _: &[u8]) -> Result<Vec<u8>, DbGatewayError> {
        Err(DbGatewayError::Execution("unused".into()))
    }
}

/// Case 4 — a call that started before the swap finishes correctly: the
/// guest parks on the gated `db_get` import, the slot upgrades meanwhile,
/// and the parked call still returns its result (it kept the old slot's
/// `Arc`).
#[tokio::test]
async fn call_started_before_the_swap_finishes_correctly_on_its_slot() {
    let engine = auto_engine();
    let gate = Arc::new(GatedOnceDb {
        gated: AtomicBool::new(false),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let function = Arc::new(
        WasmFunction::from_wat_with_mode(
            Arc::clone(&engine),
            GATED_DB_CHILD_WAT,
            WasmLimits::default(),
            TierMode::Auto,
        )
        .expect("gated guest must compile"),
    );
    assert_eq!(function.tier(), Tier::Fast);

    let ctx = FnCtx::new().with_db(Arc::clone(&gate) as Arc<dyn DbGateway>, "r".to_owned());
    let params = Params::new();

    // Park one call inside the guest's gated db_get import.
    let parked = {
        let function = Arc::clone(&function);
        let ctx = ctx.clone();
        let params = params.clone();
        tokio::spawn(async move { function.call(&ctx, &FnBatch::new(), &params).await })
    };
    timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .expect("the guest never reached the gated db_get import");

    // Cross the threshold with unblocked calls; the upgrade swaps the slot
    // while the first call is still parked on the old one.
    for _ in 0..HOT_CALLS {
        function
            .call(&ctx, &FnBatch::new(), &Params::new())
            .await
            .expect("unblocked call must succeed");
    }
    wait_for_tier(&function, Tier::Optimized).await;

    gate.release.notify_one();
    let result = timeout(Duration::from_secs(10), parked)
        .await
        .expect("the parked call never finished after release")
        .expect("parked task must not panic")
        .expect("parked call must succeed");
    assert_eq!(
        result,
        QueryValue::Map(Default::default()),
        "the call that started before the swap returned a wrong result"
    );
    assert_eq!(
        engine
            .tier_stats()
            .upgrades_completed
            .load(Ordering::Relaxed),
        1,
        "exactly one upgrade must have installed the optimized slot"
    );
}

/// Case 12 — dropping the function while its upgrade is queued behind a
/// held permit (`background_jobs == 1`): nothing is installed,
/// no failure is recorded, and the counters show the queued job never completed.
#[tokio::test]
async fn dropped_function_with_a_queued_upgrade_behind_a_held_permit_installs_nothing() {
    let engine = engine_with(HOT_CALLS, 1);
    let function = Arc::new(
        WasmFunction::from_wat_with_mode(
            Arc::clone(&engine),
            &identity_wat(),
            WasmLimits::default(),
            TierMode::Auto,
        )
        .expect("identity guest must compile"),
    );
    assert_eq!(function.tier(), Tier::Fast);

    // Occupy the single permit so the upgrade job parks in the queue.
    let permit = engine
        .upgrade_permits()
        .acquire()
        .await
        .expect("the fresh semaphore must grant a permit");
    assert_eq!(engine.upgrade_permits().available_permits(), 0);

    for _ in 0..HOT_CALLS {
        call_once(&function, &echo_params()).await;
    }
    let stats = engine.tier_stats();
    wait_until("the upgrade was never scheduled", || {
        stats.upgrades_started.load(Ordering::Relaxed) == 1
    })
    .await;

    // Drop the function while its upgrade waits for the permit...
    drop(function);
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(permit);
    // ...and wait for the queued job to run to its `Weak::upgrade`
    // discovery: the semaphore is FIFO, so this acquire is granted only after
    // the queued job released its permit, i.e. after it finished.
    let _after_job = timeout(Duration::from_secs(10), engine.upgrade_permits().acquire())
        .await
        .expect("the queued upgrade job never finished")
        .expect("the semaphore must stay open");

    assert_eq!(
        stats.upgrades_started.load(Ordering::Relaxed),
        1,
        "exactly one upgrade was queued"
    );
    assert_eq!(
        stats.upgrades_completed.load(Ordering::Relaxed),
        0,
        "a dropped function must not receive an installed upgrade"
    );
    assert_eq!(
        stats.upgrades_failed.load(Ordering::Relaxed),
        0,
        "the dropped function's job must neither fail nor panic"
    );
}
