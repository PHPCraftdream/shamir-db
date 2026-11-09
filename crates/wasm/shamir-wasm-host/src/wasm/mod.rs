//! Wasmtime execution backend for user-defined functions.
//!
//! Re-exports the public surface:
//! * [`WasmEngine`] / [`WasmLimits`] — the two-tier engine configuration (`engine`).
//! * [`TierOptions`] / [`Tier`] / [`TierMode`] / [`TierStats`] /
//!   [`TierReport`] / [`UpgradeState`] — two-tier switches (`engine`).
//! * [`WasmFunction`] — the [`ShamirFunction`] implementation (`wasm_function`).
//! * [`admit_module`] / [`check_module`] / [`AdmissionPolicy`] / [`AdmissionLimits`] / [`ModuleReport`] /
//!   [`AdmissionError`] / [`ModuleHash`] — module admission (`admission`).
//! * [`create_shamir_source`] / [`check_shamir_source`] — the shamir source
//!   service: compile `shamir-lang` source under one gate permit
//!   (`admission::gate::lang_compile`).
//! * [`verify_wasm_module`] / [`SANCTIONED_HOST_IMPORTS`] — import allowlist verification.

mod admission;
pub(crate) mod bindings;
pub(crate) mod compile_cache;
mod engine;
mod fuel;
mod wasm_function;

pub use admission::{
    admit_module, check_module, check_shamir_source, create_shamir_source, verify_wasm_module,
    AdmissionError, AdmissionLimits, AdmissionPolicy, ModuleHash, ModuleReport,
    SANCTIONED_HOST_IMPORTS,
};
pub use engine::{
    Tier, TierMode, TierOptions, TierReport, TierStats, UpgradeState, WasmEngine,
    WasmEngineOptions, WasmLimits,
};
#[cfg(test)]
pub(crate) use wasm_function::encode_params;
pub use wasm_function::WasmFunction;

/// Test-only re-export: lets
/// `tests::wasm::admission::sanitizer_tests::sanctioned_list_matches_linker_registrations`
/// build the real registered [`wasmtime::Linker`] surface (see
/// `wasm_function::test_linker_and_store` for why) and enumerate it via
/// [`wasmtime::Linker::iter`].
#[cfg(test)]
pub(crate) use wasm_function::test_linker_and_store;

/// Test-only re-export: lets `tests::wasm::fuel::*` attach and read the
/// per-invocation fuel-lease observer (including its `lease_raii_probe`
/// helper for the reservation token's refund/defuse discipline).
#[cfg(test)]
pub(crate) use fuel::FuelObserver;

/// Test-only re-export: the fault-injection switches the two-tier engine
/// tests drive through `WasmEngine::faults()` (see `engine::wasm_engine`).
#[cfg(test)]
pub(crate) use engine::TierFaults;

/// Re-export: lets `compile::compile_rust_source_blocking` offload the
/// synchronous pipeline via the shared `spawn_blocking` helper, and
/// `tests::runtime::compile_tests` exercise its `JoinError` mapping path.
pub(crate) use wasm_function::run_compile_blocking;
