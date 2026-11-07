#![forbid(unsafe_code)]

//! Module admission for S.H.A.M.I.R. wasm functions: one `wasmparser` pass that
//! parses, validates the admitted feature set, scans the import allowlist and
//! the structure, and (under [`AdmissionPolicy::Admission`]) enforces the
//! numeric caps and the export contract.
//!
//! Pure and runtime-free: no `wasmtime`, no `tokio`. The compile gate and the
//! engine stay in `shamir-wasm-host`, which re-exports this surface.

mod abi;
mod admit;
mod sanitizer;
mod scan;
mod types;

pub use abi::{
    ABI_SECTION, EXPORT_ALLOC, EXPORT_CALL, EXPORT_MEMORY, MAX_ORIGIN_BYTES, ORIGIN_SECTION,
    SUPPORTED_ABI_VERSION,
};
pub use admit::admit_module;
pub use sanitizer::{verify_wasm_module, SANCTIONED_HOST_IMPORTS};
pub use types::{AdmissionError, AdmissionLimits, AdmissionPolicy, ModuleHash, ModuleReport};

#[cfg(test)]
mod tests;
