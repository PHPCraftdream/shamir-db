//! Shared fixtures for the admission tests: contract-valid WAT modules with
//! spliced extras, `admit`/`load` wrappers and custom-section appending.

use crate::{admit_module, AdmissionError, AdmissionLimits, AdmissionPolicy, ModuleReport};

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

/// Valid contract module whose memory declares `pages` initial pages.
pub(crate) fn module_with_pages(pages: u32) -> Vec<u8> {
    assemble(&wat_text("", "", pages))
}

/// Admission under the given limits.
pub(crate) fn admit(bytes: &[u8], limits: AdmissionLimits) -> Result<ModuleReport, AdmissionError> {
    admit_module(bytes, &AdmissionPolicy::Admission(limits))
}

/// Admission under the `Load` policy.
pub(crate) fn load(bytes: &[u8]) -> Result<ModuleReport, AdmissionError> {
    admit_module(bytes, &AdmissionPolicy::Load)
}

/// Default limits adjusted by `f`.
pub(crate) fn limits_with(f: impl FnOnce(&mut AdmissionLimits)) -> AdmissionLimits {
    let mut limits = AdmissionLimits::default();
    f(&mut limits);
    limits
}

/// Report of the bare contract module under the default limits.
pub(crate) fn probe() -> ModuleReport {
    admit(&module(""), AdmissionLimits::default()).unwrap()
}

/// `usize` to `u64` for comparing against error fields.
pub(crate) fn wide(n: usize) -> u64 {
    u64::try_from(n).unwrap()
}

/// `u64` report count to `usize` for building limits.
pub(crate) fn narrow(n: u64) -> usize {
    usize::try_from(n).unwrap()
}

fn push_leb128(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let byte = u8::try_from(value & 0x7f).unwrap();
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Appends a custom section (id 0, LEB128 size, name, payload) to a module.
pub(crate) fn append_custom_section(bytes: &[u8], name: &str, payload: &[u8]) -> Vec<u8> {
    let mut section = Vec::new();
    push_leb128(&mut section, name.len());
    section.extend_from_slice(name.as_bytes());
    section.extend_from_slice(payload);
    let mut out = bytes.to_vec();
    out.push(0);
    push_leb128(&mut out, section.len());
    out.extend_from_slice(&section);
    out
}
