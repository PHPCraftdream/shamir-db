//! Two-tier Wasmtime engine pair and tier bookkeeping.
//!
//! Re-exports the module surface:
//! * [`WasmEngine`] / [`WasmEngineOptions`] / [`WasmLimits`] — engine configuration
//!   (`wasm_engine`).
//! * [`TierOptions`] / [`Tier`] / [`TierMode`] / [`TierStats`] — two-tier switches.

pub(crate) mod tier;
pub(crate) mod tier_mode;
pub(crate) mod tier_options;
pub(crate) mod tier_stats;
pub(crate) mod tier_upgrade;
mod wasm_engine;

pub use tier::Tier;
pub use tier_mode::TierMode;
pub use tier_options::TierOptions;
pub use tier_stats::TierStats;
pub use wasm_engine::{WasmEngine, WasmEngineOptions, WasmLimits};
