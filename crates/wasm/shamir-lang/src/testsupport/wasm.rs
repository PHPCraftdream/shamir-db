//! Assembles the fixed M1 runtime module for host execution tests.
//!
//! The module is deterministic: no static bytes and the default runtime
//! caps, so every host-side test drives the exact same binary.

/// The M1 runtime module bytes: `emit_runtime_module(&[], default caps)`,
/// re-encoded exports `memory` / `shamir_alloc` / `shamir_call`, with the
/// `shamir_host::raise_error` import.
pub fn runtime_module() -> Vec<u8> {
    crate::back::emit_runtime_module(&[], crate::RuntimeCaps::default())
        .expect("M1 runtime module emission with default caps")
}
