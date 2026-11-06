//! The function slot and the background tier-upgrade scheduler.
//!
//! A call resolves the slot exactly once and keeps the returned `Arc` for the
//! whole invocation, so a concurrent upgrade never disturbs an in-flight
//! call. The scheduler swaps the slot for an optimized-tier one after the
//! call counter crosses `TierOptions::hot_calls`; the job runs on tokio's
//! blocking pool, bounded by a `tokio::sync::Semaphore` owned by
//! [`WasmEngine`], and holds only a `Weak` to the function state so a
//! dropped function never keeps a compile running for nobody.

#[cfg(test)]
use super::TierFaults;
use super::{Tier, TierMode, TierReport, UpgradeState, WasmEngine, WasmLimits};
use crate::runtime::error::{FnResult, FunctionError};
use crate::wasm::wasm_function::HostState;
use arc_swap::ArcSwap;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Weak};
use std::time::Instant;
use wasmtime::{Engine, InstancePre, Module};

/// One extra upgrade attempt after a failed first one, on the next
/// threshold crossing at `RETRY_FACTOR × hot_calls`.
const RETRY_FACTOR: u64 = 16;

/// The compiled form a function call resolves once per invocation: the
/// engine the module was compiled for (a `Store` and an `InstancePre` must
/// come from the same engine), the pre-instantiated module, and the tier it
/// lives on.
pub(in crate::wasm) struct FunctionSlot {
    pub(in crate::wasm) engine: Engine,
    pub(in crate::wasm) instance_pre: InstancePre<HostState>,
    pub(in crate::wasm) tier: Tier,
}

/// State shared between a [`WasmFunction`](crate::wasm::wasm_function::WasmFunction)
/// and its background upgrade job.
pub(in crate::wasm) struct FunctionShared {
    slot: ArcSwap<FunctionSlot>,
    /// Sanitized wasm bytes: the upgrade job recompiles them on the
    /// optimized engine, since a compiled `Module` cannot cross engines.
    wasm: Arc<[u8]>,
    engine: Arc<WasmEngine>,
    limits: WasmLimits,
    mode: TierMode,
    calls: AtomicU64,
    upgrade_state: AtomicU8,
    /// Builds an `InstancePre` on a given engine; injected from the
    /// `wasm_function` module where the host-import registration lives.
    build_pre: fn(&Engine, &Module) -> FnResult<InstancePre<HostState>>,
}

impl FunctionShared {
    /// Seeds the slot cell with the creation-time slot.
    pub(in crate::wasm) fn new(
        engine: Arc<WasmEngine>,
        wasm: Arc<[u8]>,
        limits: WasmLimits,
        mode: TierMode,
        slot: FunctionSlot,
        build_pre: fn(&Engine, &Module) -> FnResult<InstancePre<HostState>>,
    ) -> Self {
        let slot = ArcSwap::from_pointee(slot);
        Self {
            slot,
            wasm,
            engine,
            limits,
            mode,
            calls: AtomicU64::new(0),
            upgrade_state: AtomicU8::new(UpgradeState::Idle as u8),
            build_pre,
        }
    }

    /// Loads the current slot once; the returned `Arc` keeps this call on
    /// its slot even across a concurrent upgrade swap.
    pub(in crate::wasm) fn slot(&self) -> Arc<FunctionSlot> {
        self.slot.load_full()
    }

    pub(in crate::wasm) fn limits(&self) -> &WasmLimits {
        &self.limits
    }

    pub(in crate::wasm) fn tier(&self) -> Tier {
        self.slot().tier
    }

    /// Tier mode fixed at construction.
    pub(in crate::wasm) fn mode(&self) -> TierMode {
        self.mode
    }

    /// Snapshot of the tier, mode, call counter and upgrade state.
    pub(in crate::wasm) fn report(&self) -> TierReport {
        TierReport {
            tier: self.tier(),
            mode: self.mode,
            calls_observed: self.calls.load(Ordering::Relaxed),
            upgrade_state: UpgradeState::from_raw(self.upgrade_state.load(Ordering::Acquire)),
        }
    }

    /// Swaps the optimized slot in. A call that already loaded the old slot
    /// keeps running on it.
    fn install(&self, slot: FunctionSlot) {
        self.slot.store(Arc::new(slot));
    }
}

/// Call-path hook: counts the invocation and schedules the single upgrade
/// when the counter crosses the hot threshold (plus one 16x-threshold retry
/// after a failure). Scheduling never blocks the call: without a tokio
/// runtime on the calling thread the attempt is skipped and a later call
/// schedules it instead.
pub(in crate::wasm) fn maybe_schedule_upgrade(shared: &Arc<FunctionShared>) {
    let engine = &shared.engine;
    let options = engine.tier_options();
    if !options.enabled || shared.mode != TierMode::Auto || engine.fast_engine().is_none() {
        return;
    }
    let hot_calls = options.hot_calls;
    if hot_calls == 0 {
        return;
    }
    let state = &shared.upgrade_state;
    // Finished or in-flight machines cost one relaxed load per call: no counter traffic, no CAS.
    let current = state.load(Ordering::Relaxed);
    if current == UpgradeState::Done as u8
        || current == UpgradeState::Exhausted as u8
        || current == UpgradeState::Scheduled as u8
    {
        return;
    }
    let calls = shared.calls.fetch_add(1, Ordering::Relaxed) + 1;
    let scheduled = if calls >= hot_calls {
        state
            .compare_exchange(
                UpgradeState::Idle as u8,
                UpgradeState::Scheduled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    } else {
        false
    };
    let retry = !scheduled
        && calls >= hot_calls.saturating_mul(RETRY_FACTOR)
        && state
            .compare_exchange(
                UpgradeState::Failed as u8,
                UpgradeState::Scheduled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
    if !scheduled && !retry {
        return;
    }
    if shared.slot().tier != Tier::Fast {
        // The slot is already optimized (creation-time fallback): nothing to
        // upgrade, retire the state machine.
        state.store(UpgradeState::Done as u8, Ordering::Release);
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        // No runtime on this thread (sync caller/test): skip rather than
        // panic; revert the transition so a later call schedules the job.
        let reverted = if retry {
            UpgradeState::Failed
        } else {
            UpgradeState::Idle
        };
        state.store(reverted as u8, Ordering::Release);
        return;
    };
    engine
        .tier_stats()
        .upgrades_started
        .fetch_add(1, Ordering::Relaxed);
    let weak = Arc::downgrade(shared);
    let job_engine = Arc::clone(&shared.engine);
    handle.spawn(async move {
        // Bound the number of concurrent compile jobs off the call path.
        let _permit = job_engine.upgrade_permits().acquire().await;
        let failure_target = weak.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || upgrade_job(&weak, retry)).await {
            // A panic in the compiler or in pre-instantiation must not leave the function
            // in `Scheduled` for good: count it, warn, and leave the retry open.
            if let Some(shared) = failure_target.upgrade() {
                record_failure(&shared, retry, &format!("upgrade job panicked: {error}"));
            }
        }
    });
}

/// The background upgrade: compile the module on the optimized engine,
/// pre-instantiate, swap the slot. Runs on the blocking pool — never on the
/// call path. Failure keeps the fast slot and leaves exactly one retry.
fn upgrade_job(weak: &Weak<FunctionShared>, is_retry: bool) {
    // Take what the compile needs and drop the strong reference: a function dropped while
    // its upgrade compiles is not kept alive by that upgrade.
    let Some((engine, wasm, build_pre)) = weak.upgrade().map(|shared| {
        (
            Arc::clone(&shared.engine),
            Arc::clone(&shared.wasm),
            shared.build_pre,
        )
    }) else {
        // The function was dropped: nobody will call the compiled slot.
        return;
    };
    let started = Instant::now();
    let compiled = (|| -> FnResult<FunctionSlot> {
        // Test-only fault injection (see `TierFaults`): a failing or
        // panicking upgrade counts as failed and leaves exactly one retry,
        // never a wedged state machine.
        #[cfg(test)]
        match engine.faults().upgrade_fault.load(Ordering::Relaxed) {
            TierFaults::UPGRADE_FAIL => {
                return Err(FunctionError::Compute(
                    "injected optimized-tier compile failure".into(),
                ));
            }
            TierFaults::UPGRADE_PANIC => panic!("injected tier-upgrade panic"),
            _ => {}
        }
        let module = engine
            .compile_optimized(&wasm)
            .map_err(|e| FunctionError::Compute(format!("optimized tier compile failed: {e}")))?;
        let instance_pre = build_pre(engine.optimized_engine(), &module)?;
        Ok(FunctionSlot {
            engine: engine.optimized_engine().clone(),
            instance_pre,
            tier: Tier::Optimized,
        })
    })();
    let Some(shared) = weak.upgrade() else {
        // Dropped during the compile: discard the result.
        return;
    };
    match compiled {
        Ok(slot) => {
            shared.install(slot);
            shared
                .upgrade_state
                .store(UpgradeState::Done as u8, Ordering::Release);
            engine
                .tier_stats()
                .upgrades_completed
                .fetch_add(1, Ordering::Relaxed);
            log::info!(
                "wasm function upgraded to the optimized tier in {:?}, {} module bytes",
                started.elapsed(),
                shared.wasm.len()
            );
        }
        Err(error) => record_failure(&shared, is_retry, &error.to_string()),
    }
}

/// Failure bookkeeping shared by the ordinary error path and the panic path: the first failure
/// leaves one retry, the retry's failure ends the attempts.
fn record_failure(shared: &FunctionShared, is_retry: bool, reason: &str) {
    let next = if is_retry {
        UpgradeState::Exhausted
    } else {
        UpgradeState::Failed
    };
    shared.upgrade_state.store(next as u8, Ordering::Release);
    shared
        .engine
        .tier_stats()
        .upgrades_failed
        .fetch_add(1, Ordering::Relaxed);
    log::warn!("wasm tier upgrade failed, keeping the fast slot: {reason}");
}
