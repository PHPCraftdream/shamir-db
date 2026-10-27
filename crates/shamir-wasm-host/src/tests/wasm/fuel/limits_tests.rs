//! Scenario E — full-u64 fuel limits with NO i64 cast or clamp
//! (RV-WASM-HOST-2E9CF487), typed epoch interruption, and typed error
//! classification (including nested propagation through the `host_call` →
//! `wasmtime::Error` → outer `FunctionError` boundary). Classification
//! assertions are deterministic; epoch-interrupt TIMING is deterministic
//! too, via the test-controlled epoch kick below; the root wall-clock
//! `timeout` additionally has a fully DETERMINISTIC paused-clock
//! regression (`start_paused` + `advance()`, tokio `test-util`, dev-only).
//!
//! Production mutation that breaks the full-u64 test: re-casting the pool
//! seed / `set_fuel` amount through `i64` (the old `u64::MAX as i64 == −1`
//! or an `i64::MAX` clamp) makes the seeded grant differ from the
//! requested `limits.fuel` — the seeded-grant assertion below reads the
//! REAL seed via the test observer and fails.
//!
//! Production mutation that breaks the classification tests: stringifying
//! the child error at the `host_call` nested boundary (the old
//! `wasmtime::Error::msg(format!("call: {e}"))`) or classifying traps by
//! message text — the typed `matches!` assertions below then fail.

use super::support::{
    d_params, echo_child_wat, fn_from_wat, relay_wat, GATED_DB_CHILD_WAT, INFINITE_LOOP_WAT,
};
use crate::gateway::db_gateway::{DbGateway, DbGatewayError};
use crate::{
    FnBatch, FnCtx, FuelObserver, FunctionError, Params, ShamirFunction, WasmEngine, WasmFunction,
};
use shamir_types::types::value::QueryValue;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::sync::Notify;

/// Fixture gateway (hoisted to module scope): kicks the engine epoch at
/// the `db_get` checkpoint — the test-controlled deterministic interrupt.
struct EpochKickDb {
    engine: Arc<WasmEngine>,
    hit: AtomicBool,
}

#[async_trait::async_trait]
impl DbGateway for EpochKickDb {
    async fn get(
        &self,
        _repo: &str,
        _table: &str,
        _key: QueryValue,
    ) -> Result<Option<QueryValue>, DbGatewayError> {
        self.hit.store(true, Ordering::SeqCst);
        // Test-controlled epoch increment at a guest-verified
        // checkpoint (twice: strict and non-strict deadline compares).
        self.engine.engine().increment_epoch();
        self.engine.engine().increment_epoch();
        Ok(None)
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

/// Fixture gateway that signals `hit` and then pends FOREVER: the guest
/// parks on the gated `db_get` host import indefinitely, so ONLY the
/// top-level wall-clock `timeout` (depth 0) can end the request.
struct PendForeverDb {
    hit: AtomicBool,
}

#[async_trait::async_trait]
impl DbGateway for PendForeverDb {
    async fn get(
        &self,
        _repo: &str,
        _table: &str,
        _key: QueryValue,
    ) -> Result<Option<QueryValue>, DbGatewayError> {
        self.hit.store(true, Ordering::SeqCst);
        std::future::pending::<Result<Option<QueryValue>, DbGatewayError>>().await
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

/// Full-u64 finite guests succeed for `i64::MAX`, `i64::MAX + 1` and
/// `u64::MAX`, AND the lazily seeded pool grant equals the requested
/// `limits.fuel` exactly (NO clamp) — observed via the per-invocation
/// test instrumentation.
#[tokio::test]
async fn rv_wasm_host_2e9cf487_full_u64_seeded_pool_no_clamp_finite_guests_succeed() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    for fuel in [i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX] {
        let wf = Arc::new(
            WasmFunction::from_wat(
                engine.clone(),
                &echo_child_wat(2_000),
                crate::WasmLimits {
                    fuel,
                    ..crate::WasmLimits::default()
                },
            )
            .unwrap(),
        );
        let obs = FuelObserver::new();
        let result = wf
            .call(
                &FnCtx::new().with_fuel_observer(obs.clone()),
                &FnBatch::new(),
                &d_params(0),
            )
            .await;
        assert!(
            result.is_ok(),
            "finite guest must succeed at fuel={fuel}: {result:?}"
        );
        assert_eq!(
            obs.seeded_count(),
            1,
            "pool seeded from limits at fuel={fuel}"
        );
        assert_eq!(
            obs.seeded(),
            fuel,
            "seeded grant must be the FULL requested u64 (no i64 cast, no clamp)"
        );
    }
}

/// The production epoch/wall-clock interruption path is unchanged and
/// classifies TYPED as [`FunctionError::Interrupted`] for an infinite
/// guest (kept epoch code per owner ruling).
#[tokio::test]
async fn rv_wasm_host_2e9cf487_epoch_interrupt_is_typed_for_infinite_guest() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let wf = Arc::new(
        WasmFunction::from_wat(
            engine,
            INFINITE_LOOP_WAT,
            crate::WasmLimits {
                fuel: u64::MAX,
                wall_clock_deadline: Duration::from_millis(300),
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );
    let err = wf
        .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
        .await
        .expect_err("infinite loop must be interrupted");
    assert!(
        matches!(err, crate::FunctionError::Interrupted),
        "expected typed Interrupted, got: {err:?}"
    );
}

/// Deterministic classification: a nested child's per-Store fuel trap and
/// epoch interrupt propagate TYPED through the host_call boundary; the
/// aggregate exhaustion is its own typed variant; instantiation-phase
/// traps classify typed instead of being stringified prematurely.
#[tokio::test]
async fn typed_classification_including_nested_propagation_and_instantiation() {
    let engine = Arc::new(WasmEngine::new().unwrap());

    // (1) Aggregate exhaustion at the entry reservation.
    let child = fn_from_wat(&engine, &echo_child_wat(2_000));
    let reg = Arc::new(crate::FunctionRegistry::new());
    reg.register("child", child.clone()).unwrap();
    let err = super::support::invoke(
        &reg,
        "child",
        Some(Arc::new(std::sync::atomic::AtomicU64::new(0))),
        None,
        &d_params(0),
    )
    .await
    .expect_err("empty pool must reject the entry reservation");
    assert!(
        matches!(err, crate::FunctionError::AggregateFuelExhausted),
        "got: {err:?}"
    );

    // (2) NESTED typed fuel: the child has a small per-Store grant and the
    // parent (generous grant) calls it — the child's Trap::OutOfFuel must
    // surface at the TOP level as FunctionError::FuelExhausted, not as a
    // stringified Compute.
    let small_child = Arc::new(
        WasmFunction::from_wat(
            engine.clone(),
            &echo_child_wat(500_000),
            crate::WasmLimits {
                fuel: 1_000,
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );
    let parent = Arc::new(
        WasmFunction::from_wat(
            engine.clone(),
            &super::support::relay_wat(1_000, 1_000, "child"),
            crate::WasmLimits {
                fuel: 1_000_000_000,
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );
    let reg2 = Arc::new(crate::FunctionRegistry::new());
    reg2.register("parent", parent).unwrap();
    reg2.register("child", small_child).unwrap();
    let ctx2 = FnCtx::new().with_registry(reg2.clone()).with_depth_limit(8);
    let err =
        crate::FunctionRegistry::invoke(&reg2, "parent", &ctx2, &FnBatch::new(), &d_params(1))
            .await
            .expect_err("small child grant must exhaust");
    assert!(
        matches!(err, crate::FunctionError::FuelExhausted),
        "nested fuel cause must stay typed across host_call, got: {err:?}"
    );

    // (3) NESTED typed interrupt: the child loops forever and its epoch
    // deadline fires; the parent sees FunctionError::Interrupted.
    let inf_child = Arc::new(
        WasmFunction::from_wat(
            engine.clone(),
            INFINITE_LOOP_WAT,
            crate::WasmLimits {
                fuel: u64::MAX,
                wall_clock_deadline: Duration::from_millis(300),
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );
    // The PARENT's limit is full u64 too: its lazily seeded pool would
    // otherwise be 1e9 and the child's lease could (rate-dependently) run
    // out of FUEL before the epoch deadline — this case must exercise the
    // INTERRUPT path, not fuel exhaustion.
    let parent2 = Arc::new(
        WasmFunction::from_wat(
            engine.clone(),
            &super::support::relay_wat(1_000, 1_000, "child"),
            crate::WasmLimits {
                fuel: u64::MAX,
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );
    let reg3 = Arc::new(crate::FunctionRegistry::new());
    reg3.register("parent", parent2).unwrap();
    reg3.register("child", inf_child).unwrap();
    let ctx3 = FnCtx::new().with_registry(reg3.clone()).with_depth_limit(8);
    let err =
        crate::FunctionRegistry::invoke(&reg3, "parent", &ctx3, &FnBatch::new(), &d_params(1))
            .await
            .expect_err("infinite child must be interrupted");
    assert!(
        matches!(err, crate::FunctionError::Interrupted),
        "nested interrupt cause must stay typed across host_call, got: {err:?}"
    );

    // (4) Instantiation-phase traps classify typed: a `start` section that
    // burns fuel exhausts the tiny per-Store grant during instantiation,
    // before `shamir_call` ever runs.
    let wf = Arc::new(
        WasmFunction::from_wat(
            engine,
            &super::support::start_trap_wat(),
            crate::WasmLimits {
                fuel: 1_000,
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );
    let err = wf
        .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
        .await
        .expect_err("start-section burn must exhaust a tiny grant");
    assert!(
        matches!(err, crate::FunctionError::FuelExhausted),
        "instantiation traps must classify typed, got: {err:?}"
    );
}

/// DETERMINISTIC epoch interruption via an explicit test-controlled
/// engine epoch increment: the gateway itself holds the `Arc<WasmEngine>`
/// and calls `Engine::increment_epoch()` at the `db_get` checkpoint, then
/// returns immediately. The child's `wall_clock_deadline` is 1 ms → a
/// 1-tick per-Store deadline, so the single kick (deliberately applied
/// twice, covering both `>` and `>=` deadline comparisons) exceeds it and
/// the resumed guest traps at the post-gate loop checkpoint — typed
/// `Interrupted` at the TOP level through the parent→child host_call
/// boundary. The parent's 30 s deadline (300 ticks) is far away, so ONLY
/// the child is interrupted. The production ticker still runs and may add
/// increments — that can only accelerate the child's trap, never delay it,
/// and cannot reach the parent's 300-tick margin within the test.
#[tokio::test]
async fn deterministic_epoch_kick_interrupts_child_typed_through_host_call() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    // Child: 1 ms deadline → 1 epoch tick; gated import + post-gate loop.
    let child = Arc::new(
        WasmFunction::from_wat(
            engine.clone(),
            super::support::GATED_DB_THEN_BURN_WAT,
            crate::WasmLimits {
                fuel: 1_000_000_000,
                wall_clock_deadline: Duration::from_millis(1),
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );
    // Parent: default 30 s deadline (300 ticks) — never tripped here.
    let parent = Arc::new(
        WasmFunction::from_wat(
            engine.clone(),
            &super::support::relay_wat(1_000, 1_000, "child"),
            crate::WasmLimits {
                fuel: 1_000_000_000,
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );

    let gated = Arc::new(EpochKickDb {
        engine: engine.clone(),
        hit: std::sync::atomic::AtomicBool::new(false),
    });
    let reg = Arc::new(crate::FunctionRegistry::new());
    reg.register("parent", parent.clone()).unwrap();
    reg.register("child", child).unwrap();

    let ctx = FnCtx::new()
        .with_registry(reg)
        .with_depth_limit(8)
        .with_db(gated.clone(), "r".into());
    let err = parent
        .call(&ctx, &FnBatch::new(), &d_params(1))
        .await
        .expect_err("the kicked epoch must interrupt the 1-tick child");
    assert!(
        gated.hit.load(Ordering::SeqCst),
        "gateway checkpoint must have been reached"
    );
    assert!(
        matches!(err, crate::FunctionError::Interrupted),
        "typed Interrupted must propagate through host_call, got: {err:?}"
    );
}

/// Top-level wall-clock deadline regression (REAL short deadline). A
/// companion PAUSED-CLOCK variant below drives the same production
/// `tokio::time::timeout` deterministically:
/// `paused_clock_root_timeout_bounds_nested_chain_at_exact_expiry`.
///
/// What it proves: a nested chain (parent → child) parked FOREVER on a
/// gated `db_get` import is still bounded by the ROOT deadline — the
/// child never gets its own timeout; nested invocations run inside the
/// depth-0 future. The failure is the typed `Compute` wall-clock message,
/// NOT `Interrupted` (the epoch deadline is 30 s away — the typed /
/// non-epoch distinction as implemented). Mutation claims (kept narrow):
/// REMOVING the depth-0 `timeout` makes the future pend forever and the
/// test TIMES OUT in CI — the hang itself is the only thing this
/// real-time test detects. A PER-NESTING-LEVEL RE-ARM is NOT
/// discriminated here: each nested entry restarts the 1 s deadline, so
/// the error still arrives (late) with the SAME typed message and this
/// test passes — the re-arm detector is the paused-clock variant below.
/// Both Stores must still settle exactly once via the
/// cancellation path (`AccountedStore::Drop`).
#[tokio::test]
async fn top_level_wall_clock_deadline_bounds_nested_chain_typed_not_epoch() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    // Child: parks on the gated db_get forever (its own default 30 s epoch
    // deadline never fires — it awaits a host import, not guest code).
    let child = fn_from_wat(&engine, GATED_DB_CHILD_WAT);
    // Parent: SHORT wall-clock deadline — the root timeout under test.
    let parent = Arc::new(
        WasmFunction::from_wat(
            engine,
            &relay_wat(1_000, 1_000, "child"),
            crate::WasmLimits {
                fuel: 1_000_000_000,
                wall_clock_deadline: Duration::from_millis(1_000),
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );
    let gated = Arc::new(PendForeverDb {
        hit: AtomicBool::new(false),
    });
    let reg = Arc::new(crate::FunctionRegistry::new());
    reg.register("parent", parent).unwrap();
    reg.register("child", child).unwrap();
    let obs = FuelObserver::new();
    let ctx = FnCtx::new()
        .with_registry(reg.clone())
        .with_depth_limit(8)
        .with_db(gated.clone(), "r".into())
        .with_fuel_observer(obs.clone());
    let err = crate::FunctionRegistry::invoke(&reg, "parent", &ctx, &FnBatch::new(), &d_params(1))
        .await
        .expect_err("the root wall-clock deadline must end the parked chain");
    assert!(
        gated.hit.load(Ordering::SeqCst),
        "the chain must have reached the gated import before the deadline"
    );
    match &err {
        crate::FunctionError::Compute(msg) => {
            assert!(
                msg.contains("wall-clock deadline"),
                "expected the root wall-clock timeout message, got: {msg}"
            );
        }
        other => panic!("expected typed Compute wall-clock timeout, not epoch/other: {other:?}"),
    }
    assert!(
        !matches!(err, crate::FunctionError::Interrupted),
        "the wall-clock timeout must NOT classify as epoch interruption"
    );
    // The dropped root future settled BOTH Stores exactly once (zero fuel
    // left installed, no calibration inconsistency).
    obs.check_settled()
        .unwrap_or_else(|e| panic!("settlement invariants broken after timeout: {e}"));
    assert_eq!(
        obs.settles(),
        2,
        "parent and child each settled exactly once"
    );
}

/// Deterministic paused-clock variant of the root wall-clock deadline
/// regression (tokio `test-util`, DEV-ONLY feature of the existing tokio
/// package; production epoch/timeout code is UNCHANGED per owner ruling).
///
/// What it proves, deterministically (no real sleeps — every step is an
/// explicit `advance()` or a manual poll):
/// * at t=0 the chain starts and parks on the PARENT's gated `db_get`
///   (stage 1) — the root timer starts at t0;
/// * the stage-1 gate is released deliberately LATE (at t = deadline − 1s)
///   and the CHILD still enters its own gated `db_get` (stage 2) — the
///   nested call is admitted inside the SAME root timeout;
/// * at t = deadline − 1ms the root future is still Pending — this point
///   alone only detects EARLY expiry (any correct, re-armed or removed
///   root timer is equally Pending here); the LATE-expiry discrimination
///   is the next point;
/// * the root future resolves with the typed `Compute` wall-clock message
///   at EXACTLY the ORIGINAL expiry (t = deadline), even though the nested
///   child was entered only 1s before — a per-nesting-level RE-ARM (or
///   removal) of the root timeout would still be Pending at the original
///   deadline and fails this assertion; a REMOVED root timer makes the
///   future Pend forever, which the bounded manual poll detects
///   immediately (no CI hang). Adding a redundant per-child timeout while
///   the ROOT timer is preserved is NOT a regression and NO detection is
///   claimed for it (the docs allow it);
/// * the child has spent only ~1s (its gate was reached at deadline − 1s)
///   at the moment of expiry — far below its own per-Store deadline;
/// * typed / non-epoch distinction: the error is `FunctionError::Compute`
///   (production wall-clock message), NOT `Interrupted` (the 60s deadline
///   is 600 epoch ticks; the real ticker advances only a few ticks in the
///   test's real-time duration and cannot reach either deadline);
/// * both Stores settle exactly once via the cancellation path.
///
/// Driving: the root future is polled MANUALLY (Box::pin + `Waker::noop`)
/// so the paused runtime never auto-advances on idle, and every stage is
/// gated on a real gateway flag — no arbitrary-Pending conflation.
/// Honesty note (static analysis only — no runtime run per policy): the
/// post-`advance` observation relies on tokio `Sleep::poll` reporting
/// Ready once the virtual clock has passed the deadline when polled; if a
/// future tokio version needed an extra scheduler turn after `advance`,
/// the final single-poll assertions would flake in CI and would need a
/// bounded re-poll loop (as `poll_until_root_stage` already does).
#[tokio::test(start_paused = true)]
async fn paused_clock_root_timeout_bounds_nested_chain_at_exact_expiry() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let child = fn_from_wat(&engine, GATED_DB_CHILD_WAT);
    // Parent: 60s root deadline — large enough that the REAL epoch ticker
    // (which a paused tokio clock does not stop) cannot approach its
    // 600-tick deadline within the test's real-time duration.
    let parent = Arc::new(
        WasmFunction::from_wat(
            engine,
            super::support::DB_GATE_THEN_CALL_CHILD_WAT,
            crate::WasmLimits {
                fuel: 1_000_000_000,
                wall_clock_deadline: Duration::from_secs(60),
                ..crate::WasmLimits::default()
            },
        )
        .unwrap(),
    );
    let gated = Arc::new(TwoStageGateDb::new());
    let reg = Arc::new(crate::FunctionRegistry::new());
    reg.register("parent", parent).unwrap();
    reg.register("child", child).unwrap();
    let obs = FuelObserver::new();
    let ctx = FnCtx::new()
        .with_registry(reg.clone())
        .with_depth_limit(8)
        .with_db(gated.clone(), "r".into())
        .with_fuel_observer(obs.clone());
    let batch = FnBatch::new();
    let params = d_params(1);
    let mut root = Box::pin(crate::FunctionRegistry::invoke(
        &reg, "parent", &ctx, &batch, &params,
    ));

    // t = 0: poll until the chain parks on the FIRST gated import.
    poll_until_root_stage(&mut root, || gated.stage1_hit(), "parent stage-1 gate");

    // Almost the whole deadline passes; THEN the nested child is admitted.
    tokio::time::advance(Duration::from_secs(59)).await;
    gated.release_stage1();

    // The late nested call reaches the CHILD's gated import.
    poll_until_root_stage(&mut root, || gated.stage2_hit(), "child stage-2 gate");

    // 1 ms before the ORIGINAL expiry: the root must still be Pending.
    tokio::time::advance(Duration::from_millis(999)).await;
    {
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert!(
            matches!(root.as_mut().poll(&mut cx), Poll::Pending),
            "root must still be Pending 1 ms before the original expiry \
             (this only proves no EARLY expiry — see the test doc)"
        );
    }

    // The final 1 ms: the root expires at EXACTLY the original deadline.
    tokio::time::advance(Duration::from_millis(1)).await;
    let err = {
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        match root.as_mut().poll(&mut cx) {
            Poll::Ready(Err(e)) => e,
            Poll::Ready(Ok(v)) => panic!("parked chain must not succeed: {v:?}"),
            Poll::Pending => panic!(
                "root deadline did not fire at the original expiry — the \
                 depth-0 timeout was removed or re-armed"
            ),
        }
    };

    // Both stages were reached: the gateway checkpoint evidence.
    assert!(
        gated.stage1_hit() && gated.stage2_hit(),
        "both gated imports must have been reached before the deadline"
    );
    // Typed / non-epoch distinction (production classification unchanged).
    match &err {
        crate::FunctionError::Compute(msg) => {
            assert!(
                msg.contains("wall-clock deadline"),
                "expected the root wall-clock timeout message, got: {msg}"
            );
        }
        other => panic!("expected typed Compute wall-clock timeout, not epoch/other: {other:?}"),
    }
    assert!(
        !matches!(err, crate::FunctionError::Interrupted),
        "the wall-clock timeout must NOT classify as epoch interruption"
    );
    // The dropped root future settled BOTH Stores exactly once (zero fuel
    // left installed, no live leases).
    obs.check_settled()
        .unwrap_or_else(|e| panic!("settlement invariants broken after timeout: {e}"));
    assert_eq!(obs.settles(), 2, "parent and child settled exactly once");
}

/// Bounded manual poll: drive `root` until `stage` flips true, panicking
/// the moment the root future resolves early (the driver catches early
/// completion or a missing stage — never a CI hang); a REMOVED root
/// timer is caught by the final expiry poll instead (the future stays
/// Pending there).
fn poll_until_root_stage<F: Future<Output = Result<QueryValue, FunctionError>>>(
    root: &mut std::pin::Pin<Box<F>>,
    stage: impl Fn() -> bool,
    what: &str,
) {
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    for _ in 0..100_000 {
        match root.as_mut().poll(&mut cx) {
            Poll::Pending => {}
            Poll::Ready(r) => panic!("{what} not reached; root resolved early: {r:?}"),
        }
        if stage() {
            return;
        }
    }
    panic!("{what} never reached under the paused clock");
}

/// Gateway gating the nested chain at TWO checkpoints: the first `get`
/// (parent stage 1) pends until the test releases it via `Notify`; the
/// second `get` (child stage 2) pends forever. The single-consumer
/// `notified().await` is the documented lost-wakeup-safe Notify shape.
struct TwoStageGateDb {
    calls: AtomicUsize,
    stage1_hit: AtomicBool,
    stage2_hit: AtomicBool,
    release1: Notify,
}

impl TwoStageGateDb {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            stage1_hit: AtomicBool::new(false),
            stage2_hit: AtomicBool::new(false),
            release1: Notify::new(),
        }
    }
    fn stage1_hit(&self) -> bool {
        self.stage1_hit.load(Ordering::SeqCst)
    }
    fn stage2_hit(&self) -> bool {
        self.stage2_hit.load(Ordering::SeqCst)
    }
    fn release_stage1(&self) {
        self.release1.notify_one();
    }
}

#[async_trait::async_trait]
impl DbGateway for TwoStageGateDb {
    async fn get(
        &self,
        _repo: &str,
        _table: &str,
        _key: QueryValue,
    ) -> Result<Option<QueryValue>, DbGatewayError> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                self.stage1_hit.store(true, Ordering::SeqCst);
                self.release1.notified().await;
                Ok(None)
            }
            _ => {
                self.stage2_hit.store(true, Ordering::SeqCst);
                std::future::pending::<Result<Option<QueryValue>, DbGatewayError>>().await
            }
        }
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
