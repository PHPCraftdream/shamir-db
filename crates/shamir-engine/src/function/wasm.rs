//! Wasmtime execution backend for user-defined functions (slice 2 → slice 6).
//!
//! A [`WasmFunction`] wraps a compiled Wasmtime [`Module`] and implements the
//! [`ShamirFunction`] trait. Execution is pure — the guest has **no host
//! imports** (slice 4 adds DB-access host functions). Parameters and results
//! are marshalled across the guest's linear memory as MessagePack bytes.
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
//! For the `*_get` variants a non-zero return is `((ptr as i64) << 32) |
//! (len as i64)` where `[ptr, ptr+len)` in guest memory holds the msgpack-
//! encoded value. The host calls the guest's `shamir_alloc` to create the
//! buffer and writes into it before returning.
//!
//! Guests that do **not** import these symbols are unaffected (unused Linker
//! definitions are silently ignored by Wasmtime).
//!
//! # Host algorithm
//!
//! 1. Encode `params` as msgpack → `input_bytes`.
//! 2. Call `shamir_alloc(input_bytes.len())` → `in_ptr`.
//! 3. Write `input_bytes` into `memory[in_ptr..in_ptr+len]`.
//! 4. Call `shamir_call(in_ptr, len)` → packed `i64`.
//! 5. Split packed into `(out_ptr, out_len)`.
//! 6. Read `out_len` bytes from `memory[out_ptr..out_ptr+out_len]`.
//! 7. Decode as `QueryValue::from_bytes`.

use super::context::{BatchContext, FnBatch, FnCtx, GlobalVars};
use super::contract::ShamirFunction;
use super::error::{FnResult, FunctionError};
use super::params::Params;
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;
use wasmtime::{Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

// ── HostState ─────────────────────────────────────────────────────────

/// Per-invocation state carried inside the Wasmtime [`Store`].
///
/// Wraps [`StoreLimits`] (memory/fuel resource caps) together with the
/// shared batch context and global-variables handles so the sync host-import
/// callbacks can reach them through [`wasmtime::Caller::data`].
struct HostState {
    limits: StoreLimits,
    batch: Arc<BatchContext>,
    globals: Arc<GlobalVars>,
}

// ── WasmEngine ────────────────────────────────────────────────────────

/// A configured Wasmtime [`Engine`] with fuel enabled.
///
/// Cheap to clone-share via `Arc` — `Engine` is internally reference-counted.
#[derive(Clone)]
pub struct WasmEngine {
    engine: Engine,
}

impl WasmEngine {
    /// Create a new engine with fuel consumption enabled.
    pub fn new() -> FnResult<Self> {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
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

        let out = tokio::task::spawn_blocking(move || -> FnResult<Vec<u8>> {
            let limiter = StoreLimitsBuilder::new()
                .memory_size(limits.max_memory_bytes)
                .build();
            let state = HostState {
                limits: limiter,
                batch: batch_ctx,
                globals,
            };
            let mut store: Store<HostState> = Store::new(engine.engine(), state);
            store.limiter(|s| &mut s.limits as &mut dyn wasmtime::ResourceLimiter);
            store
                .set_fuel(limits.fuel)
                .map_err(|e| FunctionError::Compute(e.to_string()))?;

            let mut linker: Linker<HostState> = Linker::new(engine.engine());

            // Register host imports under "shamir_host". Unused definitions
            // are harmless — modules that don't import them simply ignore them.
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

            let instance = linker
                .instantiate(&mut store, &module)
                .map_err(|e| FunctionError::Compute(format!("instantiation failed: {e}")))?;

            let memory = instance
                .get_memory(&mut store, "memory")
                .ok_or_else(|| FunctionError::Compute("missing export `memory`".into()))?;

            let alloc_fn = instance
                .get_typed_func::<i32, i32>(&mut store, "shamir_alloc")
                .map_err(|e| {
                    FunctionError::Compute(format!("missing export `shamir_alloc`: {e}"))
                })?;

            let call_fn = instance
                .get_typed_func::<(i32, i32), i64>(&mut store, "shamir_call")
                .map_err(|e| {
                    FunctionError::Compute(format!("missing export `shamir_call`: {e}"))
                })?;

            let input_len = i32::try_from(input.len())
                .map_err(|_| FunctionError::Compute("input too large for i32 length".into()))?;

            // Allocate space for input in guest memory.
            let in_ptr = alloc_fn
                .call(&mut store, input_len)
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
                .call(&mut store, (in_ptr, input_len))
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

            Ok(memory.data(&store)[out_ptr..out_end].to_vec())
        })
        .await
        .map_err(|_| FunctionError::Cancelled)??;

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
