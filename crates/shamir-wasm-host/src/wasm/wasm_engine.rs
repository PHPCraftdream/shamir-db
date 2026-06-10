//! [`WasmEngine`] and [`WasmLimits`] — Wasmtime engine configuration.

use super::super::error::{FnResult, FunctionError};
use wasmtime::{Engine, InstanceAllocationStrategy, PoolingAllocationConfig};

// ── WasmEngine ────────────────────────────────────────────────────────

/// A configured Wasmtime [`Engine`] with fuel and async support enabled.
///
/// Cheap to clone-share via `Arc` — `Engine` is internally reference-counted.
///
/// # Compilation cache
///
/// The engine enables Wasmtime's built-in disk compilation cache
/// (`wasmtime::Cache`) when possible. The cache is a **node-local
/// derivative** — it is fully recoverable from the original `.wasm`
/// bytecode, never replicated between nodes, and safe to delete at any
/// time. Wasmtime automatically invalidates stale entries when the
/// engine version or compilation target changes, so no manual
/// housekeeping is needed.
///
/// If the cache cannot be initialised (missing directory, insufficient
/// permissions, unsupported platform) the engine falls back to
/// uncached compilation — correctness is never affected.
#[derive(Clone)]
pub struct WasmEngine {
    engine: Engine,
}

impl WasmEngine {
    /// Create a new engine with fuel consumption, async support, and
    /// disk compilation cache enabled.
    ///
    /// The disk cache is a **node-local derivative** recoverable from
    /// the original `.wasm` bytecode. It is not replicated between nodes.
    /// On version or target mismatch Wasmtime silently recompiles;
    /// on any cache init error the engine falls back to uncached
    /// compilation (logged at `warn` level).
    pub fn new() -> FnResult<Self> {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);

        // Explicitly enable copy-on-write memory initialisation so
        // linear-memory data segments are mapped from a CoW image
        // instead of being copied byte-by-byte on each instantiation.
        config.memory_init_cow(true);

        // Enable Wasmtime's built-in disk compilation cache.
        // `CacheConfig::new()` uses sensible defaults (OS-specific cache
        // dir, zstd compression, background cleanup worker).
        // `Cache::new()` validates the config and spawns the worker;
        // on failure we log and proceed without cache.
        match wasmtime::Cache::new(wasmtime::CacheConfig::new()) {
            Ok(cache) => {
                config.cache(Some(cache));
                log::debug!("Wasmtime disk compilation cache enabled");
            }
            Err(e) => {
                log::warn!(
                    "Wasmtime disk compilation cache unavailable, \
                     falling back to uncached compilation: {e}"
                );
            }
        }

        // ── Allocator strategy ───────────────────────────────────────
        //
        // By default, use the pooling allocator for slot reuse under
        // concurrency. Set `SHAMIR_WASM_NO_POOL=1` (any value) to
        // force the on-demand allocator — useful for A/B benchmarking.
        let use_pooling = std::env::var("SHAMIR_WASM_NO_POOL").is_err();

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
            // `max_memory_size` is set to match `WasmLimits::max_memory_bytes`
            // (64 MiB) so the pooling allocator's per-slot cap is consistent
            // with the per-Store `ResourceLimiter`.
            let pooling_config = Self::build_pooling_config();
            config.allocation_strategy(InstanceAllocationStrategy::Pooling(pooling_config));
        }

        // async_support is auto-detected in wasmtime 45; the `async` crate
        // feature enables the fiber-based runtime that allows host imports
        // to .await. Instantiation uses instantiate_async / call_async.

        let engine = if use_pooling {
            // Try creating the Engine with pooling; on any error (platform
            // constraints, address-space limits, etc.) fall back gracefully
            // to the default on-demand allocator.
            match Engine::new(&config) {
                Ok(engine) => {
                    log::info!("Wasmtime pooling allocator enabled");
                    engine
                }
                Err(pool_err) => {
                    log::warn!(
                        "Wasmtime pooling allocator unavailable, \
                         falling back to on-demand allocator: {pool_err}"
                    );
                    config.allocation_strategy(InstanceAllocationStrategy::OnDemand);
                    Engine::new(&config).map_err(|e| FunctionError::Compute(e.to_string()))?
                }
            }
        } else {
            log::info!("Wasmtime pooling allocator disabled via SHAMIR_WASM_NO_POOL env var");
            Engine::new(&config).map_err(|e| FunctionError::Compute(e.to_string()))?
        };
        Ok(Self { engine })
    }

    /// Build a [`PoolingAllocationConfig`] whose limits are compatible
    /// with [`WasmLimits::default()`] and the per-Store
    /// [`ResourceLimiter`](wasmtime::ResourceLimiter).
    fn build_pooling_config() -> PoolingAllocationConfig {
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
        // Must be ≥ WasmLimits::max_memory_bytes (64 MiB) so the
        // per-Store ResourceLimiter can grow up to its cap without
        // hitting the pool ceiling first.
        pool.max_memory_size(WasmLimits::default().max_memory_bytes);

        // Keep a few pages resident after deallocation to avoid
        // page faults on the next re-use of the same slot.
        pool.linear_memory_keep_resident(64 * 1024); // 64 KiB

        // ── Warm-slot recycling ──────────────────────────────────
        // Prefer reusing affine (warm) slots for the same module to
        // benefit from already-faulted-in pages and CoW state.
        pool.max_unused_warm_slots(32);

        pool
    }

    /// Access the underlying Wasmtime engine.
    pub fn engine(&self) -> &Engine {
        &self.engine
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
}

impl Default for WasmLimits {
    fn default() -> Self {
        Self {
            fuel: 1_000_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
        }
    }
}
