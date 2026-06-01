//! Wasmtime execution backend for user-defined functions (slice 2 → slice 8a).
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

use super::context::{BatchContext, FnBatch, FnCtx, GlobalVars};
use super::contract::ShamirFunction;
use super::error::{FnResult, FunctionError};
use super::params::Params;
use super::registry::FunctionRegistry;
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;
use wasmtime::{Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

// ── HostState ─────────────────────────────────────────────────────────

/// Per-invocation state carried inside the Wasmtime [`Store`].
///
/// Wraps [`StoreLimits`] (memory/fuel resource caps) together with the
/// shared batch context, global-variables handles, the optional function
/// registry gateway, and recursion depth tracking so host-import callbacks
/// can reach them through [`wasmtime::Caller::data`].
struct HostState {
    limits: StoreLimits,
    batch: Arc<BatchContext>,
    globals: Arc<GlobalVars>,
    registry: Option<Arc<FunctionRegistry>>,
    depth: u32,
    depth_limit: u32,
}

// ── WasmEngine ────────────────────────────────────────────────────────

/// A configured Wasmtime [`Engine`] with fuel and async support enabled.
///
/// Cheap to clone-share via `Arc` — `Engine` is internally reference-counted.
#[derive(Clone)]
pub struct WasmEngine {
    engine: Engine,
}

impl WasmEngine {
    /// Create a new engine with fuel consumption and async support enabled.
    pub fn new() -> FnResult<Self> {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        // async_support is auto-detected in wasmtime 45; the `async` crate
        // feature enables the fiber-based runtime that allows host imports
        // to .await. Instantiation uses instantiate_async / call_async.
        let engine = Engine::new(&config).map_err(|e| FunctionError::Compute(e.to_string()))?;
        Ok(Self { engine })
    }

    /// Access the underlying Wasmtime engine.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

/// Per-invocation resource limits for a [`WasmFunction`].
#[derive(Debug, Clone)]
pub struct WasmLimits {
    /// Maximum fuel units the guest may consume.
    pub fuel: u64,
    /// Maximum linear-memory size in bytes.
    pub max_memory_bytes: usize,
}

impl Default for WasmLimits {
    fn default() -> Self {
        Self {
            fuel: 1_000_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

/// A [`ShamirFunction`] backed by a compiled WebAssembly module.
///
/// Created from either WAT text ([`WasmFunction::from_wat`]) or binary
/// `.wasm` bytes ([`WasmFunction::from_binary`]).
pub struct WasmFunction {
    module: Module,
    engine: Arc<WasmEngine>,
    limits: WasmLimits,
}

impl WasmFunction {
    /// Compile a WAT text module into a `WasmFunction`.
    pub fn from_wat(engine: Arc<WasmEngine>, wat: &str, limits: WasmLimits) -> FnResult<Self> {
        let module = Module::new(engine.engine(), wat)
            .map_err(|e| FunctionError::Compute(format!("WAT compile error: {e}")))?;
        Ok(Self {
            module,
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
        Ok(Self {
            module,
            engine,
            limits,
        })
    }
}

// ── Host-import helpers ──────────────────────────────────────────────

/// Read `len` bytes from the memory data slice starting at `ptr`.
///
/// Returns a wasm trap on out-of-bounds.
fn read_guest_mem(mem_data: &[u8], ptr: i32, len: i32) -> Result<Vec<u8>, wasmtime::Error> {
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

/// Host implementation of `batch_put(key_ptr, key_len, val_ptr, val_len)`.
fn host_batch_put(
    mut caller: wasmtime::Caller<'_, HostState>,
    key_ptr: i32,
    key_len: i32,
    val_ptr: i32,
    val_len: i32,
) -> Result<(), wasmtime::Error> {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;

    let key_bytes = read_guest_mem(memory.data(&caller), key_ptr, key_len)?;
    let val_bytes = read_guest_mem(memory.data(&caller), val_ptr, val_len)?;

    let key = String::from_utf8(key_bytes)
        .map_err(|_| wasmtime::Error::msg("batch_put: key is not valid UTF-8"))?;
    let value = QueryValue::from_bytes(&val_bytes)
        .map_err(|e| wasmtime::Error::msg(format!("batch_put: value decode error: {e}")))?;

    caller.data().batch.put(key, value);
    Ok(())
}

/// Host implementation of `batch_get(key_ptr, key_len) -> i64`.
fn host_batch_get(
    mut caller: wasmtime::Caller<'_, HostState>,
    key_ptr: i32,
    key_len: i32,
) -> Result<i64, wasmtime::Error> {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;

    let key_bytes = read_guest_mem(memory.data(&caller), key_ptr, key_len)?;
    let key = String::from_utf8(key_bytes)
        .map_err(|_| wasmtime::Error::msg("batch_get: key is not valid UTF-8"))?;

    let value = match caller.data().batch.get(&key) {
        Some(v) => v,
        None => return Ok(0),
    };

    let encoded = value
        .to_bytes()
        .map_err(|e| wasmtime::Error::msg(format!("batch_get: encode error: {e}")))?;

    // Clone the Arcs we need before &mut Caller is consumed by calling alloc.
    let batch = caller.data().batch.clone();
    let globals = caller.data().globals.clone();

    let alloc_fn = caller
        .get_export("shamir_alloc")
        .and_then(|e| e.into_func())
        .ok_or_else(|| wasmtime::Error::msg("missing export `shamir_alloc`"))?;
    let alloc_typed = alloc_fn.typed::<i32, i32>(&caller)?;

    // Release the read-only borrow on Caller before the mutable call.
    let _ = memory;
    let out_len =
        i32::try_from(encoded.len()).map_err(|_| wasmtime::Error::msg("value too large"))?;
    let out_ptr = alloc_typed.call(&mut caller, out_len)?;

    if out_ptr < 0 {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned negative pointer",
        ));
    }

    // Re-acquire the memory export and write the encoded value.
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;
    let start = out_ptr as usize;
    let end = start.saturating_add(encoded.len());
    if end > memory.data_size(&caller) {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned pointer outside memory bounds",
        ));
    }
    memory.data_mut(&mut caller)[start..end].copy_from_slice(&encoded);

    // Suppress unused-variable warnings (the clones are for borrow-scope hygiene).
    let _ = (batch, globals);

    let packed = ((out_ptr as i64) << 32) | (encoded.len() as i64 & 0xFFFF_FFFF);
    Ok(packed)
}

/// Host implementation of `global_set(key_ptr, key_len, val_ptr, val_len)`.
fn host_global_set(
    mut caller: wasmtime::Caller<'_, HostState>,
    key_ptr: i32,
    key_len: i32,
    val_ptr: i32,
    val_len: i32,
) -> Result<(), wasmtime::Error> {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;

    let key_bytes = read_guest_mem(memory.data(&caller), key_ptr, key_len)?;
    let val_bytes = read_guest_mem(memory.data(&caller), val_ptr, val_len)?;

    let key = String::from_utf8(key_bytes)
        .map_err(|_| wasmtime::Error::msg("global_set: key is not valid UTF-8"))?;
    let value = QueryValue::from_bytes(&val_bytes)
        .map_err(|e| wasmtime::Error::msg(format!("global_set: value decode error: {e}")))?;

    caller.data().globals.set(key, value);
    Ok(())
}

/// Host implementation of `global_get(key_ptr, key_len) -> i64`.
fn host_global_get(
    mut caller: wasmtime::Caller<'_, HostState>,
    key_ptr: i32,
    key_len: i32,
) -> Result<i64, wasmtime::Error> {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;

    let key_bytes = read_guest_mem(memory.data(&caller), key_ptr, key_len)?;
    let key = String::from_utf8(key_bytes)
        .map_err(|_| wasmtime::Error::msg("global_get: key is not valid UTF-8"))?;

    let value = match caller.data().globals.get(&key) {
        Some(v) => v,
        None => return Ok(0),
    };

    let encoded = value
        .to_bytes()
        .map_err(|e| wasmtime::Error::msg(format!("global_get: encode error: {e}")))?;

    // Clone the Arcs we need before &mut Caller is consumed by calling alloc.
    let batch = caller.data().batch.clone();
    let globals = caller.data().globals.clone();

    let alloc_fn = caller
        .get_export("shamir_alloc")
        .and_then(|e| e.into_func())
        .ok_or_else(|| wasmtime::Error::msg("missing export `shamir_alloc`"))?;
    let alloc_typed = alloc_fn.typed::<i32, i32>(&caller)?;

    // Release the read-only borrow on Caller before the mutable call.
    let _ = memory;
    let out_len =
        i32::try_from(encoded.len()).map_err(|_| wasmtime::Error::msg("value too large"))?;
    let out_ptr = alloc_typed.call(&mut caller, out_len)?;

    if out_ptr < 0 {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned negative pointer",
        ));
    }

    // Re-acquire the memory export and write the encoded value.
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;
    let start = out_ptr as usize;
    let end = start.saturating_add(encoded.len());
    if end > memory.data_size(&caller) {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned pointer outside memory bounds",
        ));
    }
    memory.data_mut(&mut caller)[start..end].copy_from_slice(&encoded);

    let _ = (batch, globals);

    let packed = ((out_ptr as i64) << 32) | (encoded.len() as i64 & 0xFFFF_FFFF);
    Ok(packed)
}

// ── Async host import: call ───────────────────────────────────────────
//
// Borrow-dance across await:
// 1. Read name + params bytes from guest memory into owned Vecs (sync).
// 2. Clone Arc<BatchContext>, Arc<GlobalVars>, Option<Arc<FunctionRegistry>>,
//    depth, depth_limit from caller.data() into owned locals.
// 3. Drop all borrows on Caller.
// 4. .await the registry.invoke call (may take arbitrary time).
// 5. Re-acquire Caller (mutable), get memory + shamir_alloc, write result.

/// Host implementation of `call(name_ptr, name_len, params_ptr, params_len) -> i64`.
///
/// Invokes another registered function by name. The callee shares the same
/// batch context and globals. On error (depth exceeded, function not found,
/// callee trapped), the host import traps — propagating as the caller's
/// `FunctionError::Compute`.
/// Host implementation of `call(name_ptr, name_len, params_ptr, params_len) -> i64`.
///
/// Invokes another registered function by name. The callee shares the same
/// batch context and globals. On error (depth exceeded, function not found,
/// callee trapped), the host import traps — propagating as the caller's
/// `FunctionError::Compute`.
///
/// # Borrow dance across await
///
/// 1. Read `name` + `params` bytes from guest memory via `Caller` (sync).
/// 2. Clone `Arc<BatchContext>`, `Arc<GlobalVars>`, `Option<Arc<FunctionRegistry>>`,
///    `depth`, `depth_limit` from `caller.data()` into owned locals.
/// 3. Drop the `&Caller` borrow on `memory` / `data`.
/// 4. `.await` the `registry.invoke` call.
/// 5. Re-acquire `Caller` (mutable), get `memory` + `shamir_alloc`, write result.
///
/// The `Caller<'_, HostState>` is kept alive for the entire future lifetime
/// because the returned `Box<dyn Future + Send + '_>` captures it. Wasmtime
/// guarantees `Caller` is `Send` when `T: Send` and async support is active.
fn host_call(
    mut caller: wasmtime::Caller<'_, HostState>,
    (name_ptr, name_len, params_ptr, params_len): (i32, i32, i32, i32),
) -> Box<dyn std::future::Future<Output = Result<i64, wasmtime::Error>> + Send + '_> {
    Box::new(async move {
        // ── Phase 1: read inputs + clone Arcs (all sync, before any await) ──
        let name_bytes;
        let params_bytes;
        let registry;
        let batch_ctx;
        let globals;
        let next_depth;
        let depth_limit;
        {
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .ok_or_else(|| wasmtime::Error::msg("call: missing export `memory`"))?;

            name_bytes = read_guest_mem(memory.data(&caller), name_ptr, name_len)?;
            params_bytes = read_guest_mem(memory.data(&caller), params_ptr, params_len)?;

            let state = caller.data();
            registry = state.registry.clone();
            batch_ctx = state.batch.clone();
            globals = state.globals.clone();
            next_depth = state.depth.saturating_add(1);
            depth_limit = state.depth_limit;
        }
        // Borrows on caller (memory, data) are dropped. Caller itself is still alive.

        let name = String::from_utf8(name_bytes)
            .map_err(|_| wasmtime::Error::msg("call: name is not valid UTF-8"))?;

        let params_value = QueryValue::from_bytes(&params_bytes)
            .map_err(|e| wasmtime::Error::msg(format!("call: params decode error: {e}")))?;
        let params = Params::from_value(params_value)
            .map_err(|e| wasmtime::Error::msg(format!("call: params not a map: {e}")))?;

        // ── Depth check ──
        if next_depth > depth_limit {
            return Err(wasmtime::Error::msg(format!(
                "call depth limit exceeded: {next_depth} > {depth_limit}"
            )));
        }

        let reg = registry
            .ok_or_else(|| wasmtime::Error::msg(format!("call: function not found: {name}")))?;

        // ── Phase 2: await the callee ──
        let child_ctx = FnCtx::with_globals(globals)
            .with_registry(reg)
            .with_depth(next_depth)
            .with_depth_limit(depth_limit);
        let child_batch = FnBatch::with_context(batch_ctx);

        let result = child_ctx
            .registry()
            .expect("registry just set")
            .invoke(&name, &child_ctx, &child_batch, &params)
            .await
            .map_err(|e| wasmtime::Error::msg(format!("call: {e}")))?;

        // ── Phase 3: write result back into guest memory ──
        let encoded = result
            .to_bytes()
            .map_err(|e| wasmtime::Error::msg(format!("call: result encode error: {e}")))?;

        let alloc_fn = caller
            .get_export("shamir_alloc")
            .and_then(|e| e.into_func())
            .ok_or_else(|| wasmtime::Error::msg("call: missing export `shamir_alloc`"))?;
        let alloc_typed = alloc_fn.typed::<i32, i32>(&caller)?;

        let out_len =
            i32::try_from(encoded.len()).map_err(|_| wasmtime::Error::msg("result too large"))?;
        let out_ptr = alloc_typed.call_async(&mut caller, out_len).await?;

        if out_ptr < 0 {
            return Err(wasmtime::Error::msg(
                "shamir_alloc returned negative pointer",
            ));
        }

        let memory = caller
            .get_export("memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| wasmtime::Error::msg("call: missing export `memory`"))?;
        let start = out_ptr as usize;
        let end = start.saturating_add(encoded.len());
        if end > memory.data_size(&caller) {
            return Err(wasmtime::Error::msg(
                "shamir_alloc returned pointer outside memory bounds",
            ));
        }
        memory.data_mut(&mut caller)[start..end].copy_from_slice(&encoded);

        let packed = ((out_ptr as i64) << 32) | (encoded.len() as i64 & 0xFFFF_FFFF);
        Ok(packed)
    })
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
        let module = self.module.clone();
        let batch_ctx = batch.context().clone();
        let globals = ctx.globals().clone();
        let registry = ctx.registry().cloned();
        let depth = ctx.depth();
        let depth_limit = ctx.depth_limit();

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
        };
        let mut store: Store<HostState> = Store::new(engine.engine(), state);
        store.limiter(|s| &mut s.limits as &mut dyn wasmtime::ResourceLimiter);
        store
            .set_fuel(limits.fuel)
            .map_err(|e| FunctionError::Compute(e.to_string()))?;

        let mut linker: Linker<HostState> = Linker::new(engine.engine());

        // Register sync host imports under "shamir_host". Sync host funcs are
        // allowed under async_support — they just don't await.
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

        // Register the async host import for ctx.call.
        linker
            .func_wrap_async("shamir_host", "call", host_call)
            .map_err(|e| FunctionError::Compute(format!("linker call: {e}")))?;

        let instance = linker
            .instantiate_async(&mut store, &module)
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
        let out_ptr = (packed >> 32) as u32 as usize;
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

fn map_wasm_error(e: wasmtime::Error, context: &str) -> FunctionError {
    let msg = e.to_string();
    if msg.contains("fuel") {
        FunctionError::Compute("fuel exhausted".into())
    } else {
        FunctionError::Compute(format!("{context} trap: {msg}"))
    }
}
