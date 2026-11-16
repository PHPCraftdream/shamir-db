#[cfg(test)]
use super::linker::build_linker;
use super::metrics::CallMetricsEntry;
#[cfg(test)]
use super::metrics::resolve;
use crate::gateway::db_gateway::DbGateway;
use crate::gateway::net_gateway::NetGateway;
use crate::runtime::context::{BatchContext, GlobalVars};
#[cfg(test)]
use crate::runtime::error::FnResult;
use crate::runtime::registry::{ActiveCallPath, FunctionRegistry, StoredCallResolver};
#[cfg(test)]
use crate::wasm::engine::WasmEngine;
#[cfg(test)]
use crate::wasm::fuel::FuelObserver;
use shamir_collections::TFxSet;
use shamir_types::access::Actor;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use wasmtime::StoreLimits;
#[cfg(test)]
use wasmtime::{Linker, Store, StoreLimitsBuilder};

// ── HostState ─────────────────────────────────────────────────────────

/// Per-invocation state carried inside the Wasmtime [`Store`].
///
/// Wraps [`StoreLimits`] (memory/fuel resource caps) together with the
/// shared batch context, global-variables handles, the optional function
/// registry gateway, the optional DB gateway, default repo name, and
/// recursion depth tracking so host-import callbacks can reach them
/// through [`wasmtime::Caller::data`].
pub(crate) struct HostState {
    pub(in crate::wasm) limits: StoreLimits,
    pub(in crate::wasm) batch: Arc<BatchContext>,
    pub(in crate::wasm) globals: Arc<GlobalVars>,
    pub(in crate::wasm) registry: Option<Arc<FunctionRegistry>>,
    pub(in crate::wasm) stored_call_resolver: Option<Arc<dyn StoredCallResolver>>,
    pub(in crate::wasm) active_call_path: ActiveCallPath,
    pub(in crate::wasm) definer_owner: Option<Actor>,
    pub(in crate::wasm) depth: u32,
    pub(in crate::wasm) depth_limit: u32,
    /// RI-7: the invocation's actor — threaded from the parent `FnCtx` so a
    /// nested `ctx.call(...)` inherits the SAME actor instead of silently
    /// defaulting to `Actor::System` (a confused-deputy privilege-escalation
    /// primitive). Read by `host_call.rs` to rebuild the child `FnCtx`.
    pub(in crate::wasm) actor: Actor,
    pub(in crate::wasm) db: Option<Arc<dyn DbGateway>>,
    pub(in crate::wasm) repo: String,
    pub(in crate::wasm) db_name: Option<String>,
    // Per-(db,repo) telemetry entry (call duration, fuel, depth high water).
    pub(in crate::wasm) metrics: Arc<CallMetricsEntry>,
    pub(in crate::wasm) net: Option<Arc<dyn NetGateway>>,
    /// Env-var names this function may read via `global_get("env.X")`.
    /// Non-`env.` globals are ungated; a denied secret looks absent.
    pub(in crate::wasm) secret_grants: Arc<TFxSet<String>>,
    /// Exclusive fuel-lease accounting (P0.1): shared FREE-capacity pool.
    /// Consumption leaves the pool at reservation time; unused fuel is
    /// refunded on settlement.
    pub(in crate::wasm) fuel_pool: Arc<AtomicU64>,
    /// Fuel installed in this Store for the CURRENT segment (the lease
    /// amount `set_fuel` last installed); 0 while suspended in a nested
    /// host call. Segment consumption = baseline − `get_fuel()`.
    pub(in crate::wasm) fuel_baseline: AtomicU64,
    /// This Store's OWN remaining per-Store ceiling (effective limit minus
    /// this Store's own consumption; NOT reduced by nested children).
    pub(in crate::wasm) own_cap_left: AtomicU64,
    /// Test-only per-invocation fuel-lease observability (P0.1).
    #[cfg(test)]
    pub(in crate::wasm) observer: Option<Arc<FuelObserver>>,
    /// Test-only: this Store's identity in the invocation's observer.
    #[cfg(test)]
    pub(in crate::wasm) store_id: u64,
}

/// Test-only: build a bare [`Store`] wrapping a minimal [`HostState`] plus
/// the fully-registered [`Linker`], for
/// `tests::wasm::admission::sanitizer_tests::sanctioned_list_matches_linker_registrations`
/// to enumerate the linker's real `(module, name)` surface via
/// [`Linker::iter`] and cross-check it against
/// [`super::admission::SANCTIONED_HOST_IMPORTS`]. Never compiled into
/// non-test builds.
#[cfg(test)]
pub(crate) fn test_linker_and_store(
    engine: &WasmEngine,
) -> FnResult<(Linker<HostState>, Store<HostState>)> {
    let linker = build_linker(engine.engine())?;
    let state = HostState {
        limits: StoreLimitsBuilder::new().build(),
        batch: Arc::new(BatchContext::new()),
        globals: Arc::new(GlobalVars::new()),
        registry: None,
        stored_call_resolver: None,
        active_call_path: ActiveCallPath::default(),
        definer_owner: None,
        depth: 0,
        depth_limit: 0,
        actor: Actor::System,
        db: None,
        repo: String::new(),
        db_name: None,
        metrics: resolve(None, ""),
        net: None,
        secret_grants: Arc::new(TFxSet::default()),
        fuel_pool: Arc::new(AtomicU64::new(0)),
        fuel_baseline: AtomicU64::new(0),
        own_cap_left: AtomicU64::new(0),
        #[cfg(test)]
        observer: None,
        #[cfg(test)]
        store_id: 0,
    };
    let store = Store::new(engine.engine(), state);
    Ok((linker, store))
}
