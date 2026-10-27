//! [`WasmFunction`] — the [`ShamirFunction`] implementation backed by a
//! compiled Wasmtime [`Module`].
//!
//! A [`WasmFunction`] wraps a compiled Wasmtime [`Module`] and implements the
//! [`ShamirFunction`] trait. Execution uses Wasmtime's async support so host
//! imports can `.await` (e.g. `ctx.call` for function-calls-function).
//!
//! # Guest ABI
//!
//! The module **must** export:
//!
//! * `memory` — the linear memory.
//! * `shamir_alloc(len: i32) -> i32` — allocate `len` bytes in guest memory,
//!   return the starting pointer.
//! * `shamir_call(ptr: i32, len: i32) -> i64` — execute the function.
//!   Input msgpack bytes are at `[ptr, ptr+len)`. Returns a packed `i64`:
//!   `((out_ptr as i64) << 32) | (out_len as i64 & 0xFFFF_FFFF)` pointing at
//!   the msgpack-encoded result [`QueryValue`].
//!
//! # Host imports (slice 6)
//!
//! The host registers synchronous helper functions under the import module
//! `"shamir_host"` so the guest can access the batch context and global
//! variables:
//!
//! * `batch_put(key_ptr, key_len, val_ptr, val_len)` — write to batch.
//! * `batch_get(key_ptr, key_len) -> i64` — read from batch (0 = absent).
//! * `global_set(key_ptr, key_len, val_ptr, val_len)` — write a global var.
//! * `global_get(key_ptr, key_len) -> i64` — read a global var (0 = absent).
//!
//! # Host imports (slice 8a — async)
//!
//! * `call(name_ptr, name_len, params_ptr, params_len) -> i64` — invoke
//!   another registered function by name. Uses the same batch context and
//!   globals, with a depth limit to bound recursion.
//!
//! For the `*_get` variants and `call`, a non-zero return is
//! `((ptr as i64) << 32) | (len as i64)` where `[ptr, ptr+len)` in guest
//! memory holds the msgpack-encoded value. The host calls the guest's
//! `shamir_alloc` to create the buffer and writes into it before returning.
//!
//! Guests that do **not** import these symbols are unaffected (unused Linker
//! definitions are silently ignored by Wasmtime).
//!
//! # Async execution
//!
//! Since slice 8a the engine uses `Config::async_support(true)`. The four
//! original sync host imports remain sync (they are allowed under async_support
//! and never `.await`). The new `call` host import is async — it must await
//! `FunctionRegistry::invoke`.
//!
//! Guest execution is bounded three ways: per-Store FUEL (instruction count),
//! EPOCH interruption (wall-clock pre-emption of a pure-CPU guest that never
//! awaits — engine epoch ticker + per-Store epoch deadline), and a top-level
//! WALL-CLOCK deadline (`tokio::time::timeout`) bounding the TOTAL time of a
//! request across every nested `ctx.call`.
//!
//! Exclusive fuel-lease accounting (P0.1, invariant `B = F + ΣRᵢ + C`): the
//! shared counter threaded via `host_call.rs` is the FREE-capacity pool.
//! Each Store leases (reserves) its fuel from the pool BEFORE it becomes
//! executable; when a level suspends in a nested host call its executable
//! fuel is revoked and the unused capacity released so the child can lease
//! it; on settlement the Store refunds whatever remains unused. A suspended
//! ancestor retains no executable fuel, and the aggregate instruction count
//! across the whole `ctx.call` fan-out is bounded by one shared budget —
//! same as wall-clock + epoch already bound total time.

use super::bindings::batch::{host_batch_get, host_batch_put};
use super::bindings::call::host_call;
use super::bindings::db::{host_db_execute, host_db_get, host_db_insert, host_db_query};
use super::bindings::globals::{host_global_get, host_global_set};
use super::bindings::http::host_http_fetch;
#[cfg(test)]
use super::fuel::FuelObserver;
use super::fuel::{pool_refund, FuelLease};
use super::wasm_engine::{WasmEngine, WasmLimits};
use super::wasm_sanitizer::verify_wasm_module;
use crate::gateway::db_gateway::DbGateway;
use crate::gateway::net_gateway::NetGateway;
use crate::runtime::context::{BatchContext, FnBatch, FnCtx, GlobalVars};
use crate::runtime::contract::ShamirFunction;
use crate::runtime::error::{FnResult, FunctionError};
use crate::runtime::params::Params;
use crate::runtime::registry::FunctionRegistry;
use async_trait::async_trait;
use shamir_collections::TFxSet;
use shamir_types::access::Actor;
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use wasmtime::{InstancePre, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

// ── HostState ─────────────────────────────────────────────────────────

/// Per-invocation state carried inside the Wasmtime [`Store`].
///
/// Wraps [`StoreLimits`] (memory/fuel resource caps) together with the
/// shared batch context, global-variables handles, the optional function
/// registry gateway, the optional DB gateway, default repo name, and
/// recursion depth tracking so host-import callbacks can reach them
/// through [`wasmtime::Caller::data`].
pub(crate) struct HostState {
    pub(super) limits: StoreLimits,
    pub(super) batch: Arc<BatchContext>,
    pub(super) globals: Arc<GlobalVars>,
    pub(super) registry: Option<Arc<FunctionRegistry>>,
    pub(super) depth: u32,
    pub(super) depth_limit: u32,
    /// RI-7: the invocation's actor — threaded from the parent `FnCtx` so a
    /// nested `ctx.call(...)` inherits the SAME actor instead of silently
    /// defaulting to `Actor::System` (a confused-deputy privilege-escalation
    /// primitive). Read by `host_call.rs` to rebuild the child `FnCtx`.
    pub(super) actor: Actor,
    pub(super) db: Option<Arc<dyn DbGateway>>,
    pub(super) repo: String,
    pub(super) net: Option<Arc<dyn NetGateway>>,
    /// Env-var names this function may read via `global_get("env.X")`.
    /// Non-`env.` globals are ungated; a denied secret looks absent.
    pub(super) secret_grants: Arc<TFxSet<String>>,
    /// Exclusive fuel-lease accounting (P0.1): shared FREE-capacity pool.
    /// Consumption leaves the pool at reservation time; unused fuel is
    /// refunded on settlement.
    pub(super) fuel_pool: Arc<AtomicU64>,
    /// Fuel installed in this Store for the CURRENT segment (the lease
    /// amount `set_fuel` last installed); 0 while suspended in a nested
    /// host call. Segment consumption = baseline − `get_fuel()`.
    pub(super) fuel_baseline: AtomicU64,
    /// This Store's OWN remaining per-Store ceiling (effective limit minus
    /// this Store's own consumption; NOT reduced by nested children).
    pub(super) own_cap_left: AtomicU64,
    /// Test-only per-invocation fuel-lease observability (P0.1).
    #[cfg(test)]
    pub(super) observer: Option<Arc<FuelObserver>>,
    /// Test-only: this Store's identity in the invocation's observer.
    #[cfg(test)]
    pub(super) store_id: u64,
}

/// Test-only: build a bare [`Store`] wrapping a minimal [`HostState`] plus
/// the fully-registered [`Linker`], for
/// `tests::wasm_sanitizer_tests::sanctioned_list_matches_linker_registrations`
/// to enumerate the linker's real `(module, name)` surface via
/// [`Linker::iter`] and cross-check it against
/// [`super::wasm_sanitizer::SANCTIONED_HOST_IMPORTS`]. Never compiled into
/// non-test builds.
#[cfg(test)]
pub(crate) fn test_linker_and_store(
    engine: &WasmEngine,
) -> FnResult<(Linker<HostState>, Store<HostState>)> {
    let linker = build_linker(engine)?;
    let state = HostState {
        limits: StoreLimitsBuilder::new().build(),
        batch: Arc::new(BatchContext::new()),
        globals: Arc::new(GlobalVars::new()),
        registry: None,
        depth: 0,
        depth_limit: 0,
        actor: Actor::System,
        db: None,
        repo: String::new(),
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

// ── WasmFunction ─────────────────────────────────────────────────────

/// A [`ShamirFunction`] backed by a compiled WebAssembly module.
///
/// Created from either WAT text ([`WasmFunction::from_wat`]) or binary
/// `.wasm` bytes ([`WasmFunction::from_binary`]).
pub struct WasmFunction {
    instance_pre: InstancePre<HostState>,
    engine: Arc<WasmEngine>,
    limits: WasmLimits,
}

/// Build a [`Linker`] with all host imports registered, resolving nothing
/// yet against any particular module.
///
/// `pub(crate)` (rather than private) so
/// `tests::wasm_sanitizer_tests::sanctioned_list_matches_linker_registrations`
/// can build the real registered [`Linker`] surface and cross-check its
/// `(module, name)` pairs (via [`Linker::iter`]) against
/// [`super::wasm_sanitizer::SANCTIONED_HOST_IMPORTS`] — this is the
/// mechanism keeping the sanitizer's allowlist and the linker's actual
/// registrations from silently drifting apart. Split out from
/// [`build_instance_pre`] specifically so the cross-check test does not
/// need a real [`Module`] to enumerate the registered surface.
pub(crate) fn build_linker(engine: &WasmEngine) -> FnResult<Linker<HostState>> {
    let mut linker: Linker<HostState> = Linker::new(engine.engine());

    // Sync host imports under "shamir_host".
    linker
        .func_wrap("shamir_host", "batch_put", host_batch_put)
        .map_err(|e| FunctionError::Compute(format!("linker batch_put: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "batch_get", host_batch_get)
        .map_err(|e| FunctionError::Compute(format!("linker batch_get: {e}")))?;
    linker
        .func_wrap("shamir_host", "global_set", host_global_set)
        .map_err(|e| FunctionError::Compute(format!("linker global_set: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "global_get", host_global_get)
        .map_err(|e| FunctionError::Compute(format!("linker global_get: {e}")))?;

    // Async host imports.
    linker
        .func_wrap_async("shamir_host", "call", host_call)
        .map_err(|e| FunctionError::Compute(format!("linker call: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "db_get", host_db_get)
        .map_err(|e| FunctionError::Compute(format!("linker db_get: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "db_insert", host_db_insert)
        .map_err(|e| FunctionError::Compute(format!("linker db_insert: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "db_query", host_db_query)
        .map_err(|e| FunctionError::Compute(format!("linker db_query: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "db_execute", host_db_execute)
        .map_err(|e| FunctionError::Compute(format!("linker db_execute: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "http_fetch", host_http_fetch)
        .map_err(|e| FunctionError::Compute(format!("linker http_fetch: {e}")))?;

    Ok(linker)
}

/// Build a [`Linker`] with all host imports registered and pre-instantiate
/// it against the given module. The resulting [`InstancePre`] resolves
/// imports once; each subsequent `instantiate_async` only needs a fresh
/// [`Store`].
fn build_instance_pre(engine: &WasmEngine, module: &Module) -> FnResult<InstancePre<HostState>> {
    let linker = build_linker(engine)?;
    linker
        .instantiate_pre(module)
        .map_err(|e| FunctionError::Compute(format!("instantiate_pre failed: {e}")))
}

impl WasmFunction {
    /// Compile a WAT text module into a `WasmFunction`.
    ///
    /// Runs the structural pre-instantiation sanitizer
    /// ([`verify_wasm_module`]) on the WAT's binary encoding before
    /// `wasmtime::Module::new`/`instantiate_pre` ever see it — see
    /// `wasm_sanitizer` module docs for why this is a real, if modest,
    /// defense-in-depth control (fail-fast + an explicit auditable
    /// allowlist) rather than a redundant re-check of what the linker
    /// already rejects for free.
    pub fn from_wat(engine: Arc<WasmEngine>, wat: &str, limits: WasmLimits) -> FnResult<Self> {
        let wasm_bytes = wat::parse_str(wat)
            .map_err(|e| FunctionError::Compute(format!("WAT parse error: {e}")))?;
        verify_wasm_module(&wasm_bytes)?;
        let module = engine
            .compile(&wasm_bytes)
            .map_err(|e| FunctionError::Compute(format!("WAT compile error: {e}")))?;
        let instance_pre = build_instance_pre(&engine, &module)?;
        Ok(Self {
            instance_pre,
            engine,
            limits,
        })
    }

    /// Compile a `.wasm` binary into a `WasmFunction`.
    ///
    /// Runs the structural pre-instantiation sanitizer
    /// ([`verify_wasm_module`]) on the raw bytes before
    /// `wasmtime::Module::from_binary`/`instantiate_pre` ever see them —
    /// this is the path both the compile-on-DDL pipeline
    /// (`compile_rust_source`'s output) and durable-catalogue recovery
    /// (loading previously-stored `.wasm` bytes back at startup) both go
    /// through, so every production call site is covered by this single
    /// choke point.
    ///
    /// NOTE: despite the `wasm_or_cwasm_bytes` name (inherited from
    /// `wasmtime::Module::from_binary`, which accepts either raw `.wasm`
    /// or a pre-serialized `.cwasm` artifact), no call site in this
    /// codebase produces or loads `.cwasm` bytes today — `.cwasm` AOT
    /// caching is aspirational (see the crate's module docs). The
    /// sanitizer parses raw wasm section bytes via `wasmparser` and would
    /// fail to parse a genuine `.cwasm` blob; if `.cwasm` loading is ever
    /// added, this call site will need to special-case it (e.g. skip the
    /// sanitizer for a detected `.cwasm` header, relying on the fact that
    /// `.cwasm` can only be produced by our own prior `Module::serialize`
    /// call over an already-sanitized module).
    pub fn from_binary(
        engine: Arc<WasmEngine>,
        wasm_or_cwasm_bytes: &[u8],
        limits: WasmLimits,
    ) -> FnResult<Self> {
        verify_wasm_module(wasm_or_cwasm_bytes)?;
        let module = engine
            .compile(wasm_or_cwasm_bytes)
            .map_err(|e| FunctionError::Compute(format!("WASM compile error: {e}")))?;
        let instance_pre = build_instance_pre(&engine, &module)?;
        Ok(Self {
            instance_pre,
            engine,
            limits,
        })
    }
    /// Async counterpart of [`WasmFunction::from_binary`].
    ///
    /// Cranelift compilation takes tens to hundreds of milliseconds of pure
    /// CPU; async callers on the tokio runtime must not run it inline on a
    /// worker thread. This moves the (identical) synchronous
    /// [`WasmFunction::from_binary`] pipeline — sanitizer, compile,
    /// instantiation — onto `tokio::task::spawn_blocking`. Behaviour and
    /// error values are identical to `from_binary`, except that a panic or
    /// cancellation inside the blocking task maps to
    /// [`FunctionError::Compute`] (`blocking compile join failure: …`).
    pub async fn from_binary_blocking(
        engine: Arc<WasmEngine>,
        wasm_or_cwasm_bytes: &[u8],
        limits: WasmLimits,
    ) -> FnResult<Self> {
        let wasm = wasm_or_cwasm_bytes.to_vec();
        run_compile_blocking(move || Self::from_binary(engine, &wasm, limits)).await
    }
}

/// Run a CPU-bound compile step on tokio's blocking thread pool.
///
/// Shared by [`WasmFunction::from_binary_blocking`] (and unit tests):
/// moves the closure onto `tokio::task::spawn_blocking` and maps a
/// `JoinError` (panic or cancellation of the blocking task) to
/// [`FunctionError::Compute`], the same variant a failed compile produces,
/// so callers keep a single error type.
pub(crate) async fn run_compile_blocking<F, T>(f: F) -> FnResult<T>
where
    F: FnOnce() -> FnResult<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| FunctionError::Compute(format!("blocking compile join failure: {e}")))?
}

// ── Host-import helpers ──────────────────────────────────────────────

/// Read `len` bytes from the memory data slice starting at `ptr`.
///
/// Returns a wasm trap on out-of-bounds.
pub(in crate::wasm) fn read_guest_mem(
    mem_data: &[u8],
    ptr: i32,
    len: i32,
) -> Result<Vec<u8>, wasmtime::Error> {
    if ptr < 0 || len < 0 {
        return Err(wasmtime::Error::msg("negative pointer or length"));
    }
    let start = ptr as usize;
    let end = start.saturating_add(len as usize);
    if end > mem_data.len() {
        return Err(wasmtime::Error::msg(
            "host import read past end of guest memory",
        ));
    }
    Ok(mem_data[start..end].to_vec())
}

/// Helper: write a msgpack-encoded `QueryValue` back into guest memory via
/// `shamir_alloc`, returning the packed `(ptr, len)` i64. Returns `Ok(0)`
/// if `value` is `None`.
///
/// Uses explicit reborrows (`&mut *caller`) because `Caller` is `&mut` and
/// method calls consume it.
pub(in crate::wasm) async fn write_value_to_guest(
    caller: &mut wasmtime::Caller<'_, HostState>,
    value: Option<QueryValue>,
) -> Result<i64, wasmtime::Error> {
    let value = match value {
        Some(v) => v,
        None => return Ok(0),
    };
    let encoded = value
        .to_bytes()
        .map_err(|e| wasmtime::Error::msg(format!("encode error: {e}")))?;

    let alloc_fn = caller
        .get_export("shamir_alloc")
        .and_then(|e| e.into_func())
        .ok_or_else(|| wasmtime::Error::msg("missing export `shamir_alloc`"))?;
    let alloc_typed = alloc_fn.typed::<i32, i32>(&mut *caller)?;

    let out_len =
        i32::try_from(encoded.len()).map_err(|_| wasmtime::Error::msg("value too large"))?;
    let out_ptr = alloc_typed.call_async(&mut *caller, out_len).await?;

    if out_ptr < 0 {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned negative pointer",
        ));
    }

    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;
    let start = out_ptr as usize;
    let end = start.saturating_add(encoded.len());
    if end > memory.data_size(&mut *caller) {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned pointer outside memory bounds",
        ));
    }
    memory.data_mut(&mut *caller)[start..end].copy_from_slice(&encoded);

    let packed = ((out_ptr as i64) << 32) | (encoded.len() as i64 & 0xFFFF_FFFF);
    Ok(packed)
}

/// Write raw bytes into guest memory via `shamir_alloc`, returning the
/// packed `(ptr, len)` i64. The byte-slice form of `write_value_to_guest`
/// (the payload is already-encoded msgpack, e.g. a `BatchResponse`).
pub(in crate::wasm) async fn write_bytes_to_guest(
    caller: &mut wasmtime::Caller<'_, HostState>,
    bytes: &[u8],
) -> Result<i64, wasmtime::Error> {
    let alloc_fn = caller
        .get_export("shamir_alloc")
        .and_then(|e| e.into_func())
        .ok_or_else(|| wasmtime::Error::msg("missing export `shamir_alloc`"))?;
    let alloc_typed = alloc_fn.typed::<i32, i32>(&mut *caller)?;
    let out_len =
        i32::try_from(bytes.len()).map_err(|_| wasmtime::Error::msg("value too large"))?;
    let out_ptr = alloc_typed.call_async(&mut *caller, out_len).await?;
    if out_ptr < 0 {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned negative pointer",
        ));
    }
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;
    let start = out_ptr as usize;
    let end = start.saturating_add(bytes.len());
    if end > memory.data_size(&mut *caller) {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned pointer outside memory bounds",
        ));
    }
    memory.data_mut(&mut *caller)[start..end].copy_from_slice(bytes);
    let packed = ((out_ptr as i64) << 32) | (bytes.len() as i64 & 0xFFFF_FFFF);
    Ok(packed)
}

// ── ShamirFunction impl ──────────────────────────────────────────────

#[async_trait]
impl ShamirFunction for WasmFunction {
    async fn call(&self, ctx: &FnCtx, batch: &FnBatch, params: &Params) -> FnResult<QueryValue> {
        let input = QueryValue::Map(params.raw().clone())
            .to_bytes()
            .map_err(|e| FunctionError::Compute(format!("params encode error: {e}")))?
            .to_vec();

        let engine = self.engine.clone();
        let limits = self.limits.clone();
        let batch_ctx = batch.context().clone();
        let globals = ctx.globals().clone();
        let registry = ctx.registry().cloned();
        let depth = ctx.depth();
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
            depth,
            depth_limit,
            actor,
            db,
            repo,
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
        let mut store: Store<HostState> = Store::new(engine.engine(), state);
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
                let instance = self
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

/// Map a surfaced [`wasmtime::Error`] onto the typed [`FunctionError`].
pub(super) fn map_wasm_error(e: wasmtime::Error, context: &str) -> FunctionError {
    // Typed classification (P0.2): a nested `FunctionError` wrapped by
    // `host_call` via `wasmtime::Error::new` is recovered by downcast — fuel
    // / interrupt / aggregate causes survive the boundary WITHOUT
    // stringification. Wasmtime traps are classified off the downcastable
    // `wasmtime::Trap`, not matched on message text.
    if let Some(nested) = e.downcast_ref::<FunctionError>() {
        return match nested {
            FunctionError::FuelExhausted => FunctionError::FuelExhausted,
            FunctionError::AggregateFuelExhausted => FunctionError::AggregateFuelExhausted,
            FunctionError::Interrupted => FunctionError::Interrupted,
            // Ordinary compute-class failures stay string-classified.
            other => FunctionError::Compute(format!("{context}: {other}")),
        };
    }
    if let Some(trap) = e.downcast_ref::<wasmtime::Trap>() {
        return match trap {
            wasmtime::Trap::OutOfFuel => FunctionError::FuelExhausted,
            wasmtime::Trap::Interrupt => FunctionError::Interrupted,
            _ => FunctionError::Compute(format!("{context} trap: {e}")),
        };
    }
    FunctionError::Compute(format!("{context} trap: {e}"))
}
