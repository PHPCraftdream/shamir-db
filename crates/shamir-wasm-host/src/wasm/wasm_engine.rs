//! [`WasmEngine`] and [`WasmLimits`] — Wasmtime engine configuration.

use super::compile_cache::DiskCacheStore;
use crate::runtime::error::{FnResult, FunctionError};
use std::path::PathBuf;
use std::sync::Arc;
use wasmtime::{Engine, InstanceAllocationStrategy, Module, PoolingAllocationConfig};

// ── WasmEngine ────────────────────────────────────────────────────────

/// A configured Wasmtime [`Engine`] with fuel and async support enabled.
///
/// Cheap to clone-share via `Arc` — `Engine` is internally reference-counted.
///
/// # Compilation cache
///
/// Incremental compilation entries use the Wasmtime CacheStore API; disk
/// caching is explicitly enabled only by the caller for a trusted data path.
#[derive(Clone)]
pub struct WasmEngine {
    engine: Engine,
    pooling_memory_ceiling: Option<usize>,
    /// Test-only handle to observe cache hits through the public constructor.
    #[cfg(test)]
    store: Option<Arc<DiskCacheStore>>,
}

/// Engine allocator settings. Larger ceilings reserve more address space per pool slot.
#[derive(Debug, Clone)]
pub struct WasmEngineOptions {
    pub pooling_memory_ceiling: usize,
    pub pooling: bool,
}

impl Default for WasmEngineOptions {
    fn default() -> Self {
        Self {
            pooling_memory_ceiling: WasmLimits::default().max_memory_bytes,
            pooling: std::env::var_os("SHAMIR_WASM_NO_POOL").is_none(),
        }
    }
}

impl WasmEngine {
    /// Creates an engine without a persistent compilation cache.
    pub fn new() -> FnResult<Self> {
        Self::with_options(WasmEngineOptions::default())
    }

    /// Creates an engine with a best-effort, private incremental cache.
    ///
    /// `dir` is the DATA directory; the cache store derives the
    /// `<data_dir>/cache/wasm` layout itself.
    pub fn with_compile_cache(dir: PathBuf) -> FnResult<Self> {
        Self::with_compile_cache_and_options(dir, WasmEngineOptions::default())
    }

    /// Creates an engine with explicit allocator options.
    pub fn with_options(options: WasmEngineOptions) -> FnResult<Self> {
        Self::build_with_options(None, options, Self::spawn_epoch_ticker)
    }

    /// Creates a cached engine with explicit allocator options.
    pub fn with_compile_cache_and_options(
        dir: PathBuf,
        options: WasmEngineOptions,
    ) -> FnResult<Self> {
        Self::build_with_options(Some(dir), options, Self::spawn_epoch_ticker)
    }

    #[cfg(test)]
    pub(crate) fn build_with_ticker(
        cache_dir: Option<PathBuf>,
        spawn_ticker: impl FnOnce(&Engine) -> FnResult<()>,
    ) -> FnResult<Self> {
        Self::build_with_options(cache_dir, WasmEngineOptions::default(), spawn_ticker)
    }

    fn build_with_options(
        cache_dir: Option<PathBuf>,
        options: WasmEngineOptions,
        spawn_ticker: impl FnOnce(&Engine) -> FnResult<()>,
    ) -> FnResult<Self> {
        Self::build_with_engine_factory(cache_dir, options, spawn_ticker, Engine::new)
    }

    #[cfg(test)]
    pub(crate) fn build_with_failing_pool(options: WasmEngineOptions) -> FnResult<Self> {
        let mut first = true;
        Self::build_with_engine_factory(None, options, Self::spawn_epoch_ticker, |config| {
            if std::mem::take(&mut first) {
                Err(wasmtime::Error::msg(
                    "injected pooling initialization failure",
                ))
            } else {
                Engine::new(config)
            }
        })
    }

    fn build_with_engine_factory(
        cache_dir: Option<PathBuf>,
        options: WasmEngineOptions,
        spawn_ticker: impl FnOnce(&Engine) -> FnResult<()>,
        mut create_engine: impl FnMut(&wasmtime::Config) -> wasmtime::Result<Engine>,
    ) -> FnResult<Self> {
        let store = cache_dir.map(|dir| Arc::new(DiskCacheStore::new(dir)));
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);

        // Epoch interruption (finding: WASM fuel fan-out). Fuel alone cannot
        // pre-empt a pure-CPU guest that never hits a host-await, nor charge
        // for host-await wall-clock time — a CPU-bound guest with only fuel
        // limits pins its tokio worker for the whole fuel budget. With epoch
        // interruption a background ticker (see `spawn_epoch_ticker`) bumps
        // the engine epoch on a fixed cadence, and each Store's epoch deadline
        // traps a guest that overruns its wall-clock budget regardless of how
        // fuel is consumed or reset across nested calls.
        config.epoch_interruption(true);

        // Explicitly enable copy-on-write memory initialisation so
        // linear-memory data segments are mapped from a CoW image
        // instead of being copied byte-by-byte on each instantiation.
        config.memory_init_cow(true);

        // ── Allocator strategy ───────────────────────────────────────
        //
        // By default, use the pooling allocator for slot reuse under
        // concurrency. Set `SHAMIR_WASM_NO_POOL=1` (any value) to
        // force the on-demand allocator — useful for A/B benchmarking.
        let use_pooling = options.pooling;

        if use_pooling {
            // ── Pooling allocator ────────────────────────────────────
            // Pre-allocate a pool of instance / memory / table / stack
            // slots so that `instantiate_async` can reuse warm slots
            // (with CoW-reset) instead of mmap+zero-fill on every call.
            //
            // The pool sizes are deliberately modest: virtual address
            // space is pre-reserved (total_memories × memory_reservation)
            // but pages are not committed until touched. With 128 slots
            // × 6 GiB virtual reservation ≈ 768 GiB virtual — well
            // within a 64-bit address space, and RSS stays near zero
            // until actual use.
            //
            // The configured pool ceiling is checked against each function's limits.
            let pooling_config = Self::build_pooling_config(options.pooling_memory_ceiling);
            config.allocation_strategy(InstanceAllocationStrategy::Pooling(pooling_config));
        }

        if let Some(store) = &store {
            config
                .enable_incremental_compilation(store.clone())
                .map_err(|error| FunctionError::Compute(error.to_string()))?;
        }

        let (engine, pooling_memory_ceiling) = if use_pooling {
            // Try creating the Engine with pooling; on any error (platform
            // constraints, address-space limits, etc.) fall back gracefully
            // to the default on-demand allocator.
            match create_engine(&config) {
                Ok(engine) => {
                    log::info!("Wasmtime pooling allocator enabled");
                    (engine, Some(options.pooling_memory_ceiling))
                }
                Err(pool_err) => {
                    log::warn!(
                        "Wasmtime pooling allocator unavailable, \
                         falling back to on-demand allocator: {pool_err}"
                    );
                    config.allocation_strategy(InstanceAllocationStrategy::OnDemand);
                    (
                        create_engine(&config)
                            .map_err(|e| FunctionError::Compute(e.to_string()))?,
                        None,
                    )
                }
            }
        } else {
            log::info!("Wasmtime pooling allocator disabled by engine options");
            (
                create_engine(&config).map_err(|e| FunctionError::Compute(e.to_string()))?,
                None,
            )
        };
        if let Some(store) = &store {
            if let Err(error) = store.bind(&engine) {
                log::warn!("Wasm incremental cache disabled: {error}");
            }
        }
        spawn_ticker(&engine)?;
        Ok(Self {
            engine,
            pooling_memory_ceiling,
            #[cfg(test)]
            store,
        })
    }

    /// Test-only: cache hits served to this engine's disk store.
    #[cfg(test)]
    pub(crate) fn cache_hits(&self) -> u64 {
        self.store.as_ref().map_or(0, |store| store.hits())
    }

    /// Cadence at which the background task increments the engine epoch.
    /// A Store's epoch deadline is expressed in ticks; wall-clock ÷ this
    /// cadence gives the deadline count. 100 ms bounds the worst-case
    /// over-run of a CPU-bound guest to ~one tick past its budget while
    /// keeping the background wakeup rate negligible.
    pub const EPOCH_TICK: std::time::Duration = std::time::Duration::from_millis(100);

    /// Spawn a detached OS thread that increments the engine epoch on a fixed
    /// cadence, driving epoch-based interruption of over-budget guests.
    ///
    /// This MUST be a dedicated OS thread, NOT a tokio task: a pure-CPU guest
    /// pins its tokio worker for its whole run, so an epoch ticker scheduled on
    /// the same runtime would never get to run and the trap would never fire.
    /// A standalone thread ticks regardless of runtime saturation, which is the
    /// entire point of epoch interruption (it is what lets a CPU-bound guest be
    /// pre-empted when neither fuel nor the cooperative `timeout` can).
    ///
    /// The thread holds only an `EngineWeak` (wasmtime's documented
    /// epoch-ticker pattern) and exits on the first tick after the last
    /// strong `Engine` is dropped, so short-lived engines (tests, benches)
    /// neither leak a ticker thread nor keep their allocator alive.
    fn spawn_epoch_ticker(engine: &Engine) -> FnResult<()> {
        let weak = engine.weak();
        let spawn_result = std::thread::Builder::new()
            .name("wasm-epoch-ticker".into())
            .spawn(move || loop {
                std::thread::sleep(Self::EPOCH_TICK);
                match weak.upgrade() {
                    Some(engine) => engine.increment_epoch(),
                    None => break,
                }
            });
        spawn_result.map(|_| ()).map_err(|error| {
            FunctionError::Compute(format!("failed to spawn Wasm epoch ticker: {error}"))
        })
    }

    /// Build a [`PoolingAllocationConfig`] for the configured memory ceiling.
    fn build_pooling_config(ceiling: usize) -> PoolingAllocationConfig {
        let mut pool = PoolingAllocationConfig::new();

        // ── Slot counts ──────────────────────────────────────────
        // We rarely have more than a handful of concurrent WASM
        // invocations, but keep headroom for fan-out / parallel
        // queries.  128 slots is a good balance between address
        // space and concurrency capacity.
        pool.total_core_instances(128);
        pool.total_memories(128);
        pool.total_tables(128);
        pool.total_stacks(128);

        // ── Per-module limits ────────────────────────────────────
        // Our guest modules have exactly 1 memory and ≤ 1 table.
        pool.max_memories_per_module(1);
        pool.max_tables_per_module(1);

        // ── Memory size ──────────────────────────────────────────
        pool.max_memory_size(ceiling);

        // Keep a few pages resident after deallocation to avoid
        // page faults on the next re-use of the same slot.
        pool.linear_memory_keep_resident(64 * 1024); // 64 KiB

        // ── Warm-slot recycling ──────────────────────────────────
        // Prefer reusing affine (warm) slots for the same module to
        // benefit from already-faulted-in pages and CoW state.
        pool.max_unused_warm_slots(32);

        pool
    }

    /// Effective pooling ceiling, or `None` for an on-demand allocator.
    pub fn pooling_memory_ceiling(&self) -> Option<usize> {
        self.pooling_memory_ceiling
    }

    pub(crate) fn validate_limits(&self, limits: &WasmLimits) -> FnResult<()> {
        if let Some(ceiling) = self.pooling_memory_ceiling {
            if limits.max_memory_bytes > ceiling {
                return Err(FunctionError::Compute(format!(
                    "requested WASM memory limit {} bytes exceeds pooling ceiling {} bytes; configure a larger pooling_memory_ceiling or use on-demand allocation",
                    limits.max_memory_bytes, ceiling
                )));
            }
        }
        Ok(())
    }

    /// Access the underlying Wasmtime engine.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Compiles sanitized Wasm bytes with this engine.
    pub(crate) fn compile(&self, wasm: &[u8]) -> Result<Module, wasmtime::Error> {
        Module::from_binary(&self.engine, wasm)
    }
}

// ── WasmLimits ────────────────────────────────────────────────────────

/// Per-invocation resource limits for a [`WasmFunction`](super::wasm_function::WasmFunction).
#[derive(Debug, Clone)]
pub struct WasmLimits {
    /// Maximum fuel units the guest may consume.
    pub fuel: u64,
    /// Maximum linear-memory size in bytes.
    pub max_memory_bytes: usize,
    /// Wall-clock deadline for one logical request (finding: WASM aggregate
    /// fuel fan-out). Fuel is reset per nested `Store`, so nested `ctx.call`
    /// invocations can burn `(nested count) × fuel` instructions with no
    /// aggregate ceiling; worse, fuel does not charge for time spent in
    /// host-awaits, so a guest that mostly awaits (or a pure-CPU guest with
    /// no epoch interruption) can pin a worker indefinitely. This deadline
    /// bounds the TOTAL wall-clock of a request across all nested calls,
    /// enforced at the top-level (depth 0) call site.
    pub wall_clock_deadline: std::time::Duration,
}

impl Default for WasmLimits {
    fn default() -> Self {
        Self {
            fuel: 1_000_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
            // 30 s matches the batch execution-time default; generous for a
            // single function invocation while preventing a runaway guest (or
            // fan-out chain) from pinning a worker forever.
            wall_clock_deadline: std::time::Duration::from_secs(30),
        }
    }
}
