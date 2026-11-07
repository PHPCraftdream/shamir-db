//! Shared fixtures for the admission tests: contract-valid WAT modules with
//! spliced extras, the `admit` wrapper and the build-and-run helper used by
//! the allowed-feature tests.

use crate::{
    admit_module, AdmissionError, AdmissionLimits, AdmissionPolicy, FnBatch, FnCtx, ModuleReport,
    Params, ShamirFunction, Tier, TierMode, WasmEngine, WasmFunction, WasmLimits,
};
use std::sync::Arc;

/// Assembles WAT text to a binary module.
pub(crate) fn assemble(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).unwrap()
}

/// Contract module: `pages` of exported memory, `shamir_alloc`, and a
/// `shamir_call` running `call_body` (stack-neutral) and then returning a
/// one-byte msgpack integer written at 2048. `extra_items` come first, so
/// imports may be spliced in.
fn wat_text(extra_items: &str, call_body: &str, pages: u32) -> String {
    format!(
        r#"(module
  {extra_items}
  (memory (export "memory") {pages})
  (func (export "shamir_alloc") (param i32) (result i32) (i32.const 1024))
  (func (export "shamir_call") (param i32 i32) (result i64)
    {call_body}
    (i32.store8 (i32.const 2048) (i32.const 5))
    (i64.or (i64.shl (i64.const 2048) (i64.const 32)) (i64.const 1))
  )
)"#
    )
}

/// Valid contract module with `extra_items` spliced inside `(module ...)`.
pub(crate) fn module(extra_items: &str) -> Vec<u8> {
    module_with_call_body(extra_items, "")
}

/// Like [`module`], with `call_body` executed inside `shamir_call`.
pub(crate) fn module_with_call_body(extra_items: &str, call_body: &str) -> Vec<u8> {
    assemble(&wat_text(extra_items, call_body, 1))
}

/// Admission under the given limits.
pub(crate) fn admit(bytes: &[u8], limits: AdmissionLimits) -> Result<ModuleReport, AdmissionError> {
    admit_module(bytes, &AdmissionPolicy::Admission(limits))
}

/// Default limits adjusted by `f`.
pub(crate) fn limits_with(f: impl FnOnce(&mut AdmissionLimits)) -> AdmissionLimits {
    let mut limits = AdmissionLimits::default();
    f(&mut limits);
    limits
}

/// Admits, compiles under `mode` and calls the module once; returns its tier.
pub(crate) async fn build_and_run(bytes: &[u8], mode: TierMode) -> Tier {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let (function, _report) = WasmFunction::from_binary_admitted_blocking(
        engine,
        bytes,
        WasmLimits::default(),
        mode,
        AdmissionPolicy::Admission(AdmissionLimits::default()),
    )
    .await
    .unwrap();
    let result = function
        .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
        .await;
    assert!(result.is_ok(), "call failed: {result:?}");
    function.tier()
}
