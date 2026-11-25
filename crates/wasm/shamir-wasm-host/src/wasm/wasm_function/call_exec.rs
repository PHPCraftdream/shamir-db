use crate::runtime::context::{FnBatch, FnCtx};
use crate::runtime::contract::ShamirFunction;
use crate::runtime::error::{FnResult, FunctionError};
use crate::runtime::params::Params;
use crate::wasm::engine::tier_upgrade::maybe_schedule_upgrade;
use crate::wasm::engine::{TierReport, WasmEngine};
use crate::wasm::fuel::{pool_refund, FuelLease};
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use wasmtime::{Store, StoreLimitsBuilder};

use super::function::WasmFunction;
use super::guest_io::encode_params;
use super::metrics::{resolve, CallMetricsEntry};
use super::state::HostState;
#[cfg(test)]
use crate::wasm::fuel::FuelObserver;

// ── ShamirFunction impl ──────────────────────────────────────────────

#[async_trait]
impl ShamirFunction for WasmFunction {
    fn tier_report(&self) -> Option<TierReport> {
        Some(WasmFunction::tier_report(self))
    }
    fn module_hash(&self) -> Option<crate::wasm::ModuleHash> {
        self.shared.module_hash().into()
    }
    async fn call(&self, ctx: &FnCtx, batch: &FnBatch, params: &Params) -> FnResult<QueryValue> {
        let depth = ctx.depth();
        let metrics = resolve(ctx.db_name(), ctx.repo());
        metrics.raise_depth(depth);
        // Start at entry so encoding/lease errors and cancellation are observed.
        let _duration = DurationGuard::new(depth == 0, Arc::clone(&metrics));
        maybe_schedule_upgrade(&self.shared);
        let input = encode_params(params)?;

        // The slot is resolved once; a concurrent swap cannot disturb this call.
        let slot = self.shared.slot();
        let limits = self.shared.limits().clone();
        let batch_ctx = batch.context().clone();
        let globals = ctx.globals().clone();
        let registry = ctx.registry().cloned();
        let stored_call_resolver = ctx.stored_call_resolver().cloned();
        let active_call_path = ctx.active_call_path().clone();
        let definer_owner = ctx.definer_owner().cloned();
        let depth_limit = ctx.depth_limit();
        // RI-7: the invocation's actor travels from FnCtx → HostState so
        // host_call.rs can rebuild the child FnCtx with the SAME actor for a
        // nested `ctx.call(...)` (closing the confused-deputy gap).
        let actor = ctx.actor().clone();
        let db = ctx.db_gateway().cloned();
        let repo = ctx.repo().to_string();
        let net = ctx.net_gateway().cloned();
        let secret_grants = ctx.secret_grants().clone();

        // RV-WASM-HOST-2E9CF487: `WasmLimits::fuel` is a public u64 and the
        // shared accounting is FULL-u64 end to end — no i64 cast, no clamp.
        let effective_limit = limits.fuel;
        #[cfg(test)]
        let observer = ctx.fuel_observer().cloned();
        // Exclusive fuel-lease accounting (P0.1): the shared counter is FREE
        // capacity. Consumption leaves the pool at reservation time; unused fuel
        // is refunded on settlement.
        let fuel_pool = ctx.fuel_budget().cloned().unwrap_or_else(|| {
            let seeded = Arc::new(AtomicU64::new(effective_limit));
            #[cfg(test)]
            if let Some(obs) = observer.as_ref() {
                obs.record_seed(effective_limit);
            }
            seeded
        });
        // Reserve BEFORE the fuel becomes executable (RV-WASM-HOST-6F709AC2):
        // the lease is removed from the pool up-front, so an in-flight nested
        // chain can never draw more than the aggregate budget in total. The
        // RAII [`FuelLease`] refunds the reservation even if the install
        // below fails.
        let lease = FuelLease::take(&fuel_pool, effective_limit);
        if lease.amount() == 0 {
            return Err(FunctionError::AggregateFuelExhausted);
        }

        // NOTE: we no longer use spawn_blocking. With async_support enabled
        // the entire execution runs on the tokio runtime; Wasmtime suspends
        // the fiber at host-import .await points. A pure-CPU guest with no
        // awaits is now bounded by epoch interruption (engine epoch ticker +
        // per-Store epoch deadline below), and the whole request — across all
        // nested `ctx.call` invocations — is additionally bounded by a
        // top-level wall-clock `timeout` (see the depth-0 call site).
        let deadline = limits.wall_clock_deadline;
        let limiter = StoreLimitsBuilder::new()
            .memory_size(limits.max_memory_bytes)
            .build();
        let state = HostState {
            limits: limiter,
            batch: batch_ctx,
            globals,
            registry,
            stored_call_resolver,
            active_call_path,
            definer_owner,
            depth,
            depth_limit,
            actor,
            db,
            repo,
            db_name: ctx.db_name().map(str::to_owned),
            metrics,
            net,
            secret_grants,
            fuel_pool: fuel_pool.clone(),
            fuel_baseline: AtomicU64::new(lease.amount()),
            own_cap_left: AtomicU64::new(effective_limit),
            #[cfg(test)]
            observer: observer.clone(),
            #[cfg(test)]
            store_id: 0,
        };
        let mut store: Store<HostState> = Store::new(&slot.engine, state);
        store.limiter(|s| &mut s.limits as &mut dyn wasmtime::ResourceLimiter);
        // Fail closed: the [`FuelLease`] token refunds the reservation before
        // the error propagates (drop on the early-return path).
        let granted = match store.set_fuel(lease.amount()) {
            Ok(()) => lease.consume(),
            Err(e) => {
                drop(lease);
                return Err(FunctionError::Compute(e.to_string()));
            }
        };
        store.data().fuel_baseline.store(granted, Ordering::Relaxed);
        #[cfg(test)]
        if let Some(obs) = observer.as_ref() {
            let store_id = obs.next_store_id();
            store.data_mut().store_id = store_id;
            // Actual fuel observed right after the install: the record
            // pairs RESERVATION vs INSTALLED, so a clamp/cast bug in the
            // install path is visible per Store.
            let actual = store.get_fuel().unwrap_or(0);
            obs.record_install(store_id, granted, actual, effective_limit);
        }

        // Per-Store epoch deadline: trap the guest once the engine epoch has
        // advanced past `ceil(deadline / EPOCH_TICK)` ticks. This pre-empts a
        // pure-CPU guest that never awaits (fuel exhaustion alone cannot, and
        // fuel is reset per nested Store). `set_epoch_deadline` defaults to a
        // trap on overrun, surfaced as a wasm error mapped to `FunctionError`.
        let tick_ms = WasmEngine::EPOCH_TICK.as_millis().max(1) as u64;
        let deadline_ticks = (deadline.as_millis() as u64).div_ceil(tick_ms).max(1);
        store.set_epoch_deadline(deadline_ticks);

        // Wrap the instantiate/call body in an async block. The Store is
        // owned by `AccountedStore`, whose `Drop`/`settle` refunds the
        // Store's remaining fuel to the free pool exactly once, on every
        // exit path (success, any early `?`/`Err`, or cancellation of the
        // returned future, RV-WASM-HOST-8AFCE04E).
        let result: FnResult<QueryValue> = async {
            let mut accounted = AccountedStore {
                pool: fuel_pool.clone(),
                store: Some(store),
                settled: false,
                #[cfg(test)]
                observer,
            };
            let raw: FnResult<Vec<u8>> = async {
                let store = accounted.store();
                let instance = slot
                    .instance_pre
                    .instantiate_async(&mut *store)
                    .await
                    .map_err(|e| map_wasm_error(e, "instantiation failed"))?;

                let memory = instance
                    .get_memory(&mut *store, "memory")
                    .ok_or_else(|| FunctionError::Compute("missing export `memory`".into()))?;

                let alloc_fn = instance
                    .get_typed_func::<i32, i32>(&mut *store, "shamir_alloc")
                    .map_err(|e| {
                        FunctionError::Compute(format!("missing export `shamir_alloc`: {e}"))
                    })?;

                let call_fn = instance
                    .get_typed_func::<(i32, i32), i64>(&mut *store, "shamir_call")
                    .map_err(|e| {
                        FunctionError::Compute(format!("missing export `shamir_call`: {e}"))
                    })?;

                let input_len = i32::try_from(input.len())
                    .map_err(|_| FunctionError::Compute("input too large for i32 length".into()))?;

                // Allocate space for input in guest memory.
                let in_ptr = alloc_fn
                    .call_async(&mut *store, input_len)
                    .await
                    .map_err(|e| map_wasm_error(e, "shamir_alloc"))?;
                if in_ptr < 0 {
                    return Err(FunctionError::Compute(
                        "shamir_alloc returned negative pointer".into(),
                    ));
                }
                let in_ptr_u = in_ptr as usize;
                let in_end = in_ptr_u.saturating_add(input.len());
                if in_end > memory.data_size(&*store) {
                    return Err(FunctionError::Compute(
                        "shamir_alloc pointer outside memory bounds".into(),
                    ));
                }

                // Write input msgpack into guest memory.
                memory.data_mut(&mut *store)[in_ptr_u..in_end].copy_from_slice(&input);

                // Call the guest function. At the top level (depth 0) bound the TOTAL
                // wall-clock of the request — spanning every nested `ctx.call` — with
                // a `timeout`. Nested invocations (depth > 0) run inside this future,
                // so a single top-level deadline covers the whole fan-out; re-arming
                // it per nested Store would multiply the effective budget. Epoch
                // interruption handles the pure-CPU case even when the future never
                // yields to let the `timeout` fire.
                let call_future = call_fn.call_async(&mut *store, (in_ptr, input_len));
                let packed = if depth == 0 {
                    match tokio::time::timeout(deadline, call_future).await {
                        Ok(res) => res.map_err(|e| map_wasm_error(e, "shamir_call"))?,
                        Err(_) => {
                            return Err(FunctionError::Compute(format!(
                                "wasm request exceeded wall-clock deadline ({}s)",
                                deadline.as_secs_f64()
                            )));
                        }
                    }
                } else {
                    call_future
                        .await
                        .map_err(|e| map_wasm_error(e, "shamir_call"))?
                };

                // Unpack the result pointer/length.
                let out_ptr = ((packed as u64) >> 32) as usize;
                let out_len = (packed & 0xFFFF_FFFF) as u32 as usize;
                let out_end = out_ptr.saturating_add(out_len);
                if out_end > memory.data_size(&*store) {
                    return Err(FunctionError::Compute(
                        "result pointer outside memory bounds".into(),
                    ));
                }

                let out = memory.data(&*store)[out_ptr..out_end].to_vec();
                Ok(out)
            }
            .await;
            let out = raw?;
            QueryValue::from_bytes(&out)
                .map_err(|e| FunctionError::Compute(format!("result decode error: {e}")))
            // `accounted` drops here on every path (success, `?`, or future
            // cancellation) and settles (refunds unused fuel) exactly once.
        }
        .await;

        result
    }
}

/// Exclusive fuel lease together with the Store it is installed in
/// (P0.1 exclusive-lease design). The lease left the pool at reservation
/// time, so settlement refunds ONLY the Store's remaining fuel — consumed
/// fuel was already charged. `Drop` runs on success, error, and
/// cancellation of the call future alike (RV-WASM-HOST-8AFCE04E), and is
/// idempotent.
struct AccountedStore {
    pool: Arc<AtomicU64>,
    store: Option<Store<HostState>>,
    settled: bool,
    #[cfg(test)]
    observer: Option<Arc<FuelObserver>>,
}

impl AccountedStore {
    fn store(&mut self) -> &mut Store<HostState> {
        self.store.as_mut().expect("store present until settle")
    }

    /// Refund the Store's remaining fuel to the free pool exactly once.
    fn settle(&mut self) {
        if self.settled {
            return;
        }
        if let Some(store) = self.store.as_mut() {
            let baseline = store.data().fuel_baseline.load(Ordering::Relaxed);
            let remaining = store.get_fuel().unwrap_or(0);
            let spent = baseline.saturating_sub(remaining);
            // Fuel consumed by this Store's final segment (suspend spends are recorded at the suspend site).
            store.data().metrics.add_fuel(spent);
            let _ = store.set_fuel(0);
            // Settlement owns this Store's closing bookkeeping: reset the
            // segment baseline to 0 and charge the final segment against
            // the Store's OWN remaining cap.
            store.data().fuel_baseline.store(0, Ordering::Relaxed);
            let cap = store.data().own_cap_left.load(Ordering::Relaxed);
            store
                .data()
                .own_cap_left
                .store(cap.saturating_sub(spent), Ordering::Relaxed);
            pool_refund(&self.pool, remaining);
            #[cfg(test)]
            if let Some(obs) = self.observer.as_ref() {
                // Observe the REAL fuel after the clear (must be 0) and
                // record this Store's final segment consumption.
                let after = store.get_fuel().unwrap_or(0);
                obs.record_settle(store.data().store_id, baseline, remaining, after);
            }
        }
        self.settled = true;
    }
}

impl Drop for AccountedStore {
    fn drop(&mut self) {
        self.settle();
    }
}

/// Observes the depth-0 wall-clock duration on drop — covers every
/// exit path of `call`, including early errors and cancellation.
struct DurationGuard {
    started: Option<Instant>,
    metrics: Arc<CallMetricsEntry>,
}

impl DurationGuard {
    fn new(depth0: bool, metrics: Arc<CallMetricsEntry>) -> Self {
        Self {
            started: depth0.then(Instant::now),
            metrics,
        }
    }
}

impl Drop for DurationGuard {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            let ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
            self.metrics.observe_duration_ns(ns);
        }
    }
}

/// Map a surfaced [`wasmtime::Error`] onto the typed [`FunctionError`].
pub(in crate::wasm) fn map_wasm_error(e: wasmtime::Error, context: &str) -> FunctionError {
    // Typed classification (P0.2): a nested `FunctionError` wrapped by
    // `host_call` via `wasmtime::Error::new` is recovered by downcast — fuel
    // / interrupt / aggregate causes survive the boundary WITHOUT
    // stringification. Wasmtime traps are classified off the downcastable
    // `wasmtime::Trap`, not matched on message text.
    if let Some(nested) = e.downcast_ref::<FunctionError>() {
        return match nested {
            FunctionError::User(m) => FunctionError::User(m.clone()),
            FunctionError::FuelExhausted => FunctionError::FuelExhausted,
            FunctionError::AggregateFuelExhausted => FunctionError::AggregateFuelExhausted,
            FunctionError::Interrupted => FunctionError::Interrupted,
            // A gateway refusal keeps BOTH its server-minted code and (with
            // the historical `trap:` prefix the raw host-import error used to
            // get) its rendered message, so a nested stored call's refusal
            // reaches the outer invoke without stringification.
            FunctionError::DbRefused { code, message } => FunctionError::DbRefused {
                code: code.clone(),
                message: format!("{context} trap: {message}"),
            },
            // Ordinary compute-class failures stay string-classified.
            other => FunctionError::Compute(format!("{context}: {other}")),
        };
    }
    // `{e:#}` keeps the cause chain: a host-import error (e.g. a gateway
    // permission denial) is the root cause behind the wasm backtrace context.
    if let Some(trap) = e.downcast_ref::<wasmtime::Trap>() {
        return match trap {
            wasmtime::Trap::OutOfFuel => FunctionError::FuelExhausted,
            wasmtime::Trap::Interrupt => FunctionError::Interrupted,
            _ => FunctionError::Compute(format!("{context} trap: {e:#}")),
        };
    }
    FunctionError::Compute(format!("{context} trap: {e:#}"))
}
