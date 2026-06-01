//! Wasmtime execution backend for user-defined functions (slice 2).
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
//! # Host algorithm
//!
//! 1. Encode `params` as msgpack → `input_bytes`.
//! 2. Call `shamir_alloc(input_bytes.len())` → `in_ptr`.
//! 3. Write `input_bytes` into `memory[in_ptr..in_ptr+len]`.
//! 4. Call `shamir_call(in_ptr, len)` → packed `i64`.
//! 5. Split packed into `(out_ptr, out_len)`.
//! 6. Read `out_len` bytes from `memory[out_ptr..out_ptr+out_len]`.
//! 7. Decode as `QueryValue::from_bytes`.

use super::context::{FnBatch, FnCtx};
use super::contract::ShamirFunction;
use super::error::{FnResult, FunctionError};
use super::params::Params;
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;
use wasmtime::{Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

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

#[async_trait]
impl ShamirFunction for WasmFunction {
    async fn call(&self, _ctx: &FnCtx, _batch: &FnBatch, params: &Params) -> FnResult<QueryValue> {
        let input = QueryValue::Map(params.raw().clone())
            .to_bytes()
            .map_err(|e| FunctionError::Compute(format!("params encode error: {e}")))?
            .to_vec();

        let engine = self.engine.clone();
        let limits = self.limits.clone();
        let module = self.module.clone();

        let out = tokio::task::spawn_blocking(move || -> FnResult<Vec<u8>> {
            let limiter = StoreLimitsBuilder::new()
                .memory_size(limits.max_memory_bytes)
                .build();
            let mut store: Store<StoreLimits> = Store::new(engine.engine(), limiter);
            store.limiter(|s| s as &mut dyn wasmtime::ResourceLimiter);
            store
                .set_fuel(limits.fuel)
                .map_err(|e| FunctionError::Compute(e.to_string()))?;

            let linker: Linker<StoreLimits> = Linker::new(engine.engine());
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
