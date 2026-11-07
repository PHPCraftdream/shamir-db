//! [`TierOptions`] — two-tier compilation switches in `WasmEngineOptions`.

/// Two-tier compilation settings carried in `WasmEngineOptions::tier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierOptions {
    /// Master switch: `false` builds a single-tier (optimized) engine.
    pub enabled: bool,
    /// Call count after which a function is upgraded to the optimized tier.
    pub hot_calls: u64,
    /// Upper bound on concurrently running background upgrade compiles.
    pub background_jobs: usize,
}

impl Default for TierOptions {
    /// Design defaults: tiers on, `hot_calls = 64`, one job per four cores.
    fn default() -> Self {
        Self {
            enabled: true,
            hot_calls: 64,
            background_jobs: std::thread::available_parallelism()
                .map_or(1, |cores| cores.get() / 4)
                .max(1),
        }
    }
}
