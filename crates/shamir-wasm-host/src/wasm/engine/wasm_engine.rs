//! [`WasmEngine`] and [`WasmLimits`] — Wasmtime engine configuration for the
//! two-tier compilation pair (fast Winch + optimized Cranelift).

use super::{TierOptions, TierStats};
use crate::runtime::error::{FnResult, FunctionError};
use crate::wasm::admission::AdmissionLimits;
use crate::wasm::compile_cache::DiskCacheStore;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;
use wasmtime::{Engine, InstanceAllocationStrategy, Module, PoolingAllocationConfig};

/// Bench-only (cargo feature `bench-strategies`, never enabled by the server build):
/// `SHAMIR_WASM_STRATEGY` picks the code generator for an A/B run of the same suite -
/// `cranelift` (default), `winch` (single-pass baseline compiler) or `pulley` (portable
/// interpreter). Anything else is rejected. The switch applies to the OPTIMIZED engine
/// only; the fast engine is always Winch.
#[cfg(feature = "bench-strategies")]
fn apply_bench_strategy(config: &mut wasmtime::Config) -> FnResult<()> {
    const STRATEGY_ENV: &str = "SHAMIR_WASM_STRATEGY";
    match std::env::var(STRATEGY_ENV).ok().as_deref() {
        None | Some("" | "cranelift") => {}
        Some("winch") => {
            config.strategy(wasmtime::Strategy::Winch);
        }
        Some("pulley") => {
            config
                .target("pulley64")
                .map_err(|error| FunctionError::Compute(error.to_string()))?;
        }
        Some(other) => {
            return Err(FunctionError::Compute(format!(
                "{STRATEGY_ENV}=`{other}`: expected cranelift, winch or pulley"
            )));
        }
    }
    Ok(())
}

/// Test-only fault injection for the two-tier paths (never compiled into
/// non-test builds). Each engine owns its own switches, so parallel tests
/// on distinct engines cannot interfere.
///
/// * `fail_fast_compile` — [`WasmEngine::compile_fast`] fails with an
///   injected error, forcing the creation-time `Auto` fallback to the
///   optimized tier without needing a module the Winch backend rejects.
/// * `upgrade_fault` — the background upgrade job fails
///   ([`TierFaults::UPGRADE_FAIL`]) or panics ([`TierFaults::UPGRADE_PANIC`])
///   instead of compiling, exercising the failure/retry/exhaustion machine.
/// * `compile_delay_ms`, `compile_active`, `compile_max_active` — the
///   compile-gate probe: an injected per-job delay plus the current and peak
///   number of gated compile jobs inside their blocking section.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct TierFaults {
    /// Forces the fast-tier compile to fail.
    pub(crate) fail_fast_compile: AtomicBool,
    /// What the background upgrade job does instead of compiling.
    pub(crate) upgrade_fault: AtomicU8,
    /// Milliseconds every gated blocking compile job sleeps after taking its
    /// permit.
    pub(crate) compile_delay_ms: AtomicU64,
    /// Gated jobs currently inside their blocking section.
    pub(crate) compile_active: AtomicUsize,
    /// Highest value `compile_active` reached.
    pub(crate) compile_max_active: AtomicUsize,
}

#[cfg(test)]
impl TierFaults {
    /// Upgrade job compiles normally.
    pub(crate) const UPGRADE_NONE: u8 = 0;
    /// Upgrade job fails with an injected compile error.
    pub(crate) const UPGRADE_FAIL: u8 = 1;
    /// Upgrade job panics (exercises the `JoinError` failure path).
    pub(crate) const UPGRADE_PANIC: u8 = 2;

    /// Marks a gated compile job's blocking section: counts it active,
    /// records the peak and sleeps the injected delay; dropping the probe
    /// leaves the section.
    pub(crate) fn enter_compile(&self) -> CompileProbe<'_> {
        let active = self.compile_active.fetch_add(1, Ordering::SeqCst) + 1;
        self.compile_max_active.fetch_max(active, Ordering::SeqCst);
        let delay = self.compile_delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        CompileProbe(self)
    }
}

/// Guard of a gated compile job's blocking section; dropping it leaves the
/// section.
#[cfg(test)]
pub(crate) struct CompileProbe<'a>(&'a TierFaults);

#[cfg(test)]
impl Drop for CompileProbe<'_> {
    fn drop(&mut self) {
        self.0.compile_active.fetch_sub(1, Ordering::SeqCst);
    }
}

// ── WasmEngine ────────────────────────────────────────────────────────

/// A configured pair of Wasmtime [`Engine`]s with fuel and async support
/// enabled: a **fast** engine (Winch, cheap compile) and an **optimized**
/// engine (Cranelift, the historical construction including the incremental
/// compile cache).
///
/// Cheap to clone-share via `Arc` — `Engine` is internally reference-counted.
/// A failed fast-engine build degrades to single-tier on the optimized
/// engine (logged at `warn`); everything downstream works with `fast`
/// absent.
///
/// # Compilation cache
///
/// Incremental compilation entries use the Wasmtime CacheStore API and are a
/// Cranelift feature: the cache is enabled only on the optimized engine; the
/// Winch engine compiles without it. Disk caching is explicitly enabled only
/// by the caller for a trusted data path.
#[derive(Clone)]
pub struct WasmEngine {
    /// The optimized engine (Cranelift): the historical single engine and
    /// the upgrade target of the two-tier scheme.
    optimized: Engine,
    /// The fast engine (Winch); `None` after a failed fast-engine build
    /// (single-tier fallback).
    fast: Option<Engine>,
    pooling_memory_ceiling: Option<usize>,
    tier_options: TierOptions,
    /// Bounds concurrently running background upgrade compiles
    /// (`TierOptions::background_jobs`).
    upgrade_permits: Arc<Semaphore>,
    /// Upgrade lifecycle counters shared with the background jobs.
    stats: Arc<TierStats>,
    /// Bounds concurrently running admitted compiles
    /// (`AdmissionLimits::max_concurrent_compiles`).
    compile_permits: Arc<Semaphore>,
    /// Admission gate caps this engine was built with.
    admission: AdmissionLimits,
    /// Cranelift engine without the incremental disk cache and without
    /// pooling, built on first use; the dry-run check compiles on it so it
    /// never writes the cache.
    uncached_optimized: Arc<OnceLock<Engine>>,
    /// Test-only fault-injection switches (see [`TierFaults`]).
    #[cfg(test)]
    faults: Arc<TierFaults>,
    /// Test-only handle to observe cache hits through the public constructor.
    #[cfg(test)]
    store: Option<Arc<DiskCacheStore>>,
}

/// Engine allocator settings. Larger ceilings reserve more address space per pool slot.
#[derive(Debug, Clone)]
pub struct WasmEngineOptions {
    pub pooling_memory_ceiling: usize,
    pub pooling: bool,
    /// Two-tier compilation switches (fast Winch tier + optimized Cranelift
    /// tier, background upgrade scheduling).
    pub tier: TierOptions,
    /// Caps for the admission gate set; exposed by the engine through `WasmEngine::admission_limits`.
    pub admission: AdmissionLimits,
}

impl Default for WasmEngineOptions {
    fn default() -> Self {
        Self {
            pooling_memory_ceiling: WasmLimits::default().max_memory_bytes,
            pooling: std::env::var_os("SHAMIR_WASM_NO_POOL").is_none(),
            tier: TierOptions::default(),
            admission: AdmissionLimits::default(),
        }
    }
}

impl WasmEngine {
    /// Admission gate caps this engine was built with.
    pub fn admission_limits(&self) -> &AdmissionLimits {
        &self.admission
    }

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
        Self::build_with_options(
            cache_dir,
            WasmEngineOptions::default(),
            |optimized, _fast| spawn_ticker(optimized),
        )
    }

    fn build_with_options(
        cache_dir: Option<PathBuf>,
        options: WasmEngineOptions,
        spawn_ticker: impl FnOnce(&Engine, Option<&Engine>) -> FnResult<()>,
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
        spawn_ticker: impl FnOnce(&Engine, Option<&Engine>) -> FnResult<()>,
        mut create_engine: impl FnMut(&wasmtime::Config) -> wasmtime::Result<Engine>,
    ) -> FnResult<Self> {
        let store = cache_dir.map(|dir| Arc::new(DiskCacheStore::new(dir)));

        // ── Optimized engine (Cranelift) ─────────────────────────────
        // The historical single engine: the bench-strategy switch and the
        // incremental compile cache live here.
        #[cfg(feature = "bench-strategies")]
        let mut optimized_config = Self::base_config();
        #[cfg(feature = "bench-strategies")]
        apply_bench_strategy(&mut optimized_config)?;
        #[cfg(not(feature = "bench-strategies"))]
        let optimized_config = Self::base_config();
        let (optimized, optimized_ceiling) = Self::create_configured_engine(
            optimized_config,
            options.pooling,
            options.pooling_memory_ceiling,
            store.as_ref(),
            &mut create_engine,
            "optimized",
        )?;

        // ── Fast engine (Winch) ──────────────────────────────────────
        // Single-pass baseline compiler: cheap compile, slower code. No
        // incremental cache (a Cranelift-only feature). Built only while tiering is enabled,
        // and with a pool only when the optimized engine got one, so both tiers share the same
        // allocator semantics. A failed build (unsupported target, resource limits) degrades
        // this engine to single-tier on the optimized engine - never an error.
        let fast = if options.tier.enabled {
            match Self::create_configured_engine(
                Self::fast_config(),
                optimized_ceiling.is_some(),
                options.pooling_memory_ceiling,
                None,
                &mut create_engine,
                "fast",
            ) {
                Ok((engine, _)) => Some(engine),
                Err(error) => {
                    log::warn!(
                        "Wasm fast tier (Winch) engine unavailable, \
                         running single-tier on the optimized engine: {error}"
                    );
                    None
                }
            }
        } else {
            None
        };

        // The pool-derived limit rejection follows the optimized engine: it is the one whose
        // pool bounds a function on every path (`Now`, upgrades); the fast engine pools with
        // the same ceiling or not at all.
        let pooling_memory_ceiling = optimized_ceiling;

        spawn_ticker(&optimized, fast.as_ref())?;
        Ok(Self {
            optimized,
            fast,
            pooling_memory_ceiling,
            tier_options: options.tier,
            upgrade_permits: Arc::new(Semaphore::new(options.tier.background_jobs.max(1))),
            stats: Arc::new(TierStats::default()),
            compile_permits: Arc::new(Semaphore::new(
                options.admission.max_concurrent_compiles.max(1),
            )),
            admission: options.admission,
            uncached_optimized: Arc::new(OnceLock::new()),
            #[cfg(test)]
            faults: Arc::new(TierFaults::default()),
            #[cfg(test)]
            store,
        })
    }

    /// Fuel + epoch interruption + CoW memory init — shared by both tier
    /// engines so a guest is bounded identically on either tier.
    fn base_config() -> wasmtime::Config {
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
        config
    }

    /// Fast-tier config: the single-pass Winch strategy on the shared
    /// limits configuration. `strategy` itself is infallible; an
    /// unsupported target fails later at `Engine::new`, which the caller
    /// maps to single-tier fallback.
    fn fast_config() -> wasmtime::Config {
        let mut config = Self::base_config();
        config.strategy(wasmtime::Strategy::Winch);
        config
    }

    /// Assemble one tier engine from a prepared config: optional
    /// incremental cache, pooling-with-fallback allocator, cache bind.
    /// Returns the engine and its pooling ceiling (`None` = on-demand).
    fn create_configured_engine(
        mut config: wasmtime::Config,
        pooling: bool,
        ceiling: usize,
        cache: Option<&Arc<DiskCacheStore>>,
        create_engine: &mut impl FnMut(&wasmtime::Config) -> wasmtime::Result<Engine>,
        tier: &str,
    ) -> FnResult<(Engine, Option<usize>)> {
        if let Some(store) = cache {
            config
                .enable_incremental_compilation(store.clone())
                .map_err(|error| FunctionError::Compute(error.to_string()))?;
        }

        if pooling {
            config.allocation_strategy(InstanceAllocationStrategy::Pooling(
                Self::build_pooling_config(ceiling),
            ));
        }

        let (engine, active_ceiling) = if pooling {
            // Try creating the Engine with pooling; on any error (platform
            // constraints, address-space limits, etc.) fall back gracefully
            // to the default on-demand allocator, per engine.
            match create_engine(&config) {
                Ok(engine) => {
                    log::info!("Wasmtime pooling allocator enabled ({tier})");
                    (engine, Some(ceiling))
                }
                Err(pool_err) => {
                    log::warn!(
                        "Wasmtime pooling allocator unavailable ({tier}), \
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
            log::info!("Wasmtime pooling allocator disabled by engine options ({tier})");
            (
                create_engine(&config).map_err(|e| FunctionError::Compute(e.to_string()))?,
                None,
            )
        };

        if let Some(store) = cache {
            if let Err(error) = store.bind(&engine) {
                log::warn!("Wasm incremental cache disabled: {error}");
            }
        }
        Ok((engine, active_ceiling))
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

    /// Spawn a detached OS thread that increments BOTH engines' epochs on a
    /// fixed cadence, driving epoch-based interruption of over-budget guests.
    ///
    /// This MUST be a dedicated OS thread, NOT a tokio task: a pure-CPU guest
    /// pins its tokio worker for its whole run, so an epoch ticker scheduled on
    /// the same runtime would never get to run and the trap would never fire.
    /// A standalone thread ticks regardless of runtime saturation, which is the
    /// entire point of epoch interruption (it is what lets a CPU-bound guest be
    /// pre-empted when neither fuel nor the cooperative `timeout` can).
    ///
    /// The thread holds only `EngineWeak`s (wasmtime's documented
    /// epoch-ticker pattern — one thread ticking both engines) and exits on
    /// the first tick after the last strong `Engine` is gone, so short-lived
    /// engines (tests, benches) neither leak a ticker thread nor keep their
    /// allocator alive.
    fn spawn_epoch_ticker(optimized: &Engine, fast: Option<&Engine>) -> FnResult<()> {
        let optimized_weak = optimized.weak();
        let fast_weak = fast.map(Engine::weak);
        let spawn_result = std::thread::Builder::new()
            .name("wasm-epoch-ticker".into())
            .spawn(move || loop {
                std::thread::sleep(Self::EPOCH_TICK);
                let mut alive = false;
                if let Some(engine) = optimized_weak.upgrade() {
                    engine.increment_epoch();
                    alive = true;
                }
                if let Some(engine) = fast_weak.as_ref().and_then(|weak| weak.upgrade()) {
                    engine.increment_epoch();
                    alive = true;
                }
                if !alive {
                    break;
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

    /// Pool-derived ceiling active on EVERY engine of the pair, or `None`
    /// when any engine runs on-demand (a limit is pool-rejected only when
    /// every engine that can host the function enforces the same ceiling).
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

    /// Access the optimized Wasmtime engine (the historical single engine).
    pub fn engine(&self) -> &Engine {
        &self.optimized
    }

    /// Access the optimized (Cranelift) engine.
    pub fn optimized_engine(&self) -> &Engine {
        &self.optimized
    }

    /// Access the fast (Winch) engine, or `None` after a failed fast-engine
    /// build (single-tier fallback on the optimized engine).
    pub fn fast_engine(&self) -> Option<&Engine> {
        self.fast.as_ref()
    }

    /// Two-tier switches this engine was built with.
    pub(crate) fn tier_options(&self) -> &TierOptions {
        &self.tier_options
    }

    /// Upgrade lifecycle counters shared with the background jobs (tests,
    /// telemetry).
    pub fn tier_stats(&self) -> &TierStats {
        &self.stats
    }

    /// Test-only fault-injection switches of this engine (see [`TierFaults`]).
    #[cfg(test)]
    pub(crate) fn faults(&self) -> &Arc<TierFaults> {
        &self.faults
    }

    /// Permit pool bounding concurrent background upgrade compiles.
    pub(crate) fn upgrade_permits(&self) -> &Arc<Semaphore> {
        &self.upgrade_permits
    }

    /// Permit pool bounding concurrently running admitted compiles.
    pub(crate) fn compile_permits(&self) -> &Arc<Semaphore> {
        &self.compile_permits
    }

    /// Engine for the dry-run check: Cranelift without the incremental disk
    /// cache and without pooling, built on first use, so checking a module
    /// never writes the cache.
    pub(crate) fn uncached_optimized_engine(&self) -> FnResult<&Engine> {
        if let Some(engine) = self.uncached_optimized.get() {
            return Ok(engine);
        }
        #[cfg(feature = "bench-strategies")]
        let mut config = Self::base_config();
        #[cfg(feature = "bench-strategies")]
        apply_bench_strategy(&mut config)?;
        #[cfg(not(feature = "bench-strategies"))]
        let config = Self::base_config();
        let engine =
            Engine::new(&config).map_err(|error| FunctionError::Compute(error.to_string()))?;
        Ok(self.uncached_optimized.get_or_init(|| engine))
    }

    /// Compiles sanitized Wasm bytes on the optimized (Cranelift) engine.
    pub fn compile_optimized(&self, wasm: &[u8]) -> Result<Module, wasmtime::Error> {
        Module::from_binary(&self.optimized, wasm)
    }

    /// Compiles sanitized Wasm bytes on the fast (Winch) engine. Errors when
    /// the fast engine is unavailable (single-tier fallback).
    pub fn compile_fast(&self, wasm: &[u8]) -> Result<Module, wasmtime::Error> {
        // Test-only seam (see `TierFaults`): forces the fast-tier compile to
        // fail so the creation-time fallback to the optimized tier can be
        // pinned without a module the Winch backend rejects.
        #[cfg(test)]
        if self.faults.fail_fast_compile.load(Ordering::Relaxed) {
            return Err(wasmtime::Error::msg("injected fast-tier compile failure"));
        }
        match &self.fast {
            Some(engine) => Module::from_binary(engine, wasm),
            None => Err(wasmtime::Error::msg(
                "fast tier unavailable: the fast engine failed to initialize",
            )),
        }
    }

    /// Compiles sanitized Wasm bytes on the optimized engine — kept under
    /// its historical name for the compile-cache test suite (its only
    /// callers), hence compiled only under `cfg(test)`.
    #[cfg(test)]
    pub(crate) fn compile(&self, wasm: &[u8]) -> Result<Module, wasmtime::Error> {
        self.compile_optimized(wasm)
    }
}

// ── WasmLimits ────────────────────────────────────────────────────────

/// Per-invocation resource limits for a function running on either tier.
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
