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
//! Note: a CPU-bound guest with no host-import awaits still occupies the
//! tokio worker thread for the duration of its timeslice. Epoch-based
//! yielding is a future refinement; out of scope here.

use super::super::context::{BatchContext, FnBatch, FnCtx, GlobalVars};
use super::super::contract::ShamirFunction;
use super::super::db_gateway::DbGateway;
use super::super::error::{FnResult, FunctionError};
use super::super::net_gateway::NetGateway;
use super::super::params::Params;
use super::super::registry::FunctionRegistry;
use super::host_batch::{host_batch_get, host_batch_put};
use super::host_call::host_call;
use super::host_db::{host_db_execute, host_db_get, host_db_insert, host_db_query};
use super::host_globals::{host_global_get, host_global_set};
use super::host_http::host_http_fetch;
use super::wasm_engine::{WasmEngine, WasmLimits};
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
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
pub(super) struct HostState {
    pub(super) limits: StoreLimits,
    pub(super) batch: Arc<BatchContext>,
    pub(super) globals: Arc<GlobalVars>,
    pub(super) registry: Option<Arc<FunctionRegistry>>,
    pub(super) depth: u32,
    pub(super) depth_limit: u32,
    pub(super) db: Option<Arc<dyn DbGateway>>,
    pub(super) repo: String,
    pub(super) net: Option<Arc<dyn NetGateway>>,
    /// Env-var names this function may read via `global_get("env.X")`.
    /// Non-`env.` globals are ungated; a denied secret looks absent.
    pub(super) secret_grants: Arc<std::collections::HashSet<String, shamir_collections::THasher>>,
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

/// Build a [`Linker`] with all host imports registered and pre-instantiate
/// it against the given module. The resulting [`InstancePre`] resolves
/// imports once; each subsequent `instantiate_async` only needs a fresh
/// [`Store`].
fn build_instance_pre(engine: &WasmEngine, module: &Module) -> FnResult<InstancePre<HostState>> {
    let mut linker: Linker<HostState> = Linker::new(engine.engine());

    // Sync host imports under "shamir_host".
    linker
        .func_wrap("shamir_host", "batch_put", host_batch_put)
        .map_err(|e| FunctionError::Compute(format!("linker batch_put: {e}")))?;
    linker
        .func_wrap("shamir_host", "batch_get", host_batch_get)
        .map_err(|e| FunctionError::Compute(format!("linker batch_get: {e}")))?;
    linker
        .func_wrap("shamir_host", "global_set", host_global_set)
        .map_err(|e| FunctionError::Compute(format!("linker global_set: {e}")))?;
    linker
        .func_wrap("shamir_host", "global_get", host_global_get)
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

    linker
        .instantiate_pre(module)
        .map_err(|e| FunctionError::Compute(format!("instantiate_pre failed: {e}")))
}

impl WasmFunction {
    /// Compile a WAT text module into a `WasmFunction`.
    pub fn from_wat(engine: Arc<WasmEngine>, wat: &str, limits: WasmLimits) -> FnResult<Self> {
        let module = Module::new(engine.engine(), wat)
            .map_err(|e| FunctionError::Compute(format!("WAT compile error: {e}")))?;
        let instance_pre = build_instance_pre(&engine, &module)?;
        Ok(Self {
            instance_pre,
            engine,
            limits,
        })
    }

    /// Compile a `.wasm` binary into a `WasmFunction`.
    pub fn from_binary(
        engine: Arc<WasmEngine>,
        wasm_or_cwasm_bytes: &[u8],
        limits: WasmLimits,
    ) -> FnResult<Self> {
        let module = Module::from_binary(engine.engine(), wasm_or_cwasm_bytes)
            .map_err(|e| FunctionError::Compute(format!("WASM compile error: {e}")))?;
        let instance_pre = build_instance_pre(&engine, &module)?;
        Ok(Self {
            instance_pre,
            engine,
            limits,
        })
    }
}

// ── Host-import helpers ──────────────────────────────────────────────

/// Read `len` bytes from the memory data slice starting at `ptr`.
///
/// Returns a wasm trap on out-of-bounds.
pub(super) fn read_guest_mem(
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
pub(super) async fn write_value_to_guest(
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
pub(super) async fn write_bytes_to_guest(
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
        let db = ctx.db_gateway().cloned();
        let repo = ctx.repo().to_string();
        let net = ctx.net_gateway().cloned();
        let secret_grants = ctx.secret_grants().clone();

        // NOTE: we no longer use spawn_blocking. With async_support enabled
        // the entire execution runs on the tokio runtime; Wasmtime suspends
        // the fiber at host-import .await points (only the `call` import
        // awaits today). A CPU-bound guest with no awaits still occupies the
        // worker — epoch-based yielding is a future refinement.
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
            db,
            repo,
            net,
            secret_grants,
        };
        let mut store: Store<HostState> = Store::new(engine.engine(), state);
        store.limiter(|s| &mut s.limits as &mut dyn wasmtime::ResourceLimiter);
        store
            .set_fuel(limits.fuel)
            .map_err(|e| FunctionError::Compute(e.to_string()))?;

        let instance = self
            .instance_pre
            .instantiate_async(&mut store)
            .await
            .map_err(|e| FunctionError::Compute(format!("instantiation failed: {e}")))?;

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| FunctionError::Compute("missing export `memory`".into()))?;

        let alloc_fn = instance
            .get_typed_func::<i32, i32>(&mut store, "shamir_alloc")
            .map_err(|e| FunctionError::Compute(format!("missing export `shamir_alloc`: {e}")))?;

        let call_fn = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, "shamir_call")
            .map_err(|e| FunctionError::Compute(format!("missing export `shamir_call`: {e}")))?;

        let input_len = i32::try_from(input.len())
            .map_err(|_| FunctionError::Compute("input too large for i32 length".into()))?;

        // Allocate space for input in guest memory.
        let in_ptr = alloc_fn
            .call_async(&mut store, input_len)
            .await
            .map_err(|e| map_wasm_error(e, "shamir_alloc"))?;
        if in_ptr < 0 {
            return Err(FunctionError::Compute(
                "shamir_alloc returned negative pointer".into(),
            ));
        }
        let in_ptr_u = in_ptr as usize;
        let in_end = in_ptr_u.saturating_add(input.len());
        if in_end > memory.data_size(&store) {
            return Err(FunctionError::Compute(
                "shamir_alloc pointer outside memory bounds".into(),
            ));
        }

        // Write input msgpack into guest memory.
        memory.data_mut(&mut store)[in_ptr_u..in_end].copy_from_slice(&input);

        // Call the guest function.
        let packed = call_fn
            .call_async(&mut store, (in_ptr, input_len))
            .await
            .map_err(|e| map_wasm_error(e, "shamir_call"))?;

        // Unpack the result pointer/length.
        let out_ptr = ((packed as u64) >> 32) as usize;
        let out_len = (packed & 0xFFFF_FFFF) as u32 as usize;
        let out_end = out_ptr.saturating_add(out_len);
        if out_end > memory.data_size(&store) {
            return Err(FunctionError::Compute(
                "result pointer outside memory bounds".into(),
            ));
        }

        let out = memory.data(&store)[out_ptr..out_end].to_vec();

        QueryValue::from_bytes(&out)
            .map_err(|e| FunctionError::Compute(format!("result decode error: {e}")))
    }
}

pub(super) fn map_wasm_error(e: wasmtime::Error, context: &str) -> FunctionError {
    let msg = e.to_string();
    if msg.contains("fuel") {
        FunctionError::Compute("fuel exhausted".into())
    } else {
        FunctionError::Compute(format!("{context} trap: {msg}"))
    }
}
