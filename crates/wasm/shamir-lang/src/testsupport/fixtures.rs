//! Runtime cap and admission-policy fixtures used by tests here and in the
//! host crate.

use crate::RuntimeCaps;

use shamir_wasm_admission::{AdmissionLimits, AdmissionPolicy};

/// The production-shaped cap set the emitted module carries by default.
pub fn default_caps() -> RuntimeCaps {
    RuntimeCaps::default()
}

/// A minimal cap set for tests that exercise cap-driven emission sizes
/// (tiny workspace cell region, depth 2, few entries).
pub fn tiny_caps() -> RuntimeCaps {
    RuntimeCaps {
        input_bytes: 256,
        output_bytes: 256,
        string_bytes: 64,
        key_bytes: 64,
        containers: 4,
        entries: 4,
        cells: 16,
        depth: 2,
    }
}

/// The admission policy the host execution tests admit the emitted module
/// under, mirroring `back::emit::tests::module_tests::admission_policy`:
/// the module must fit the 64 KiB runtime code budget.
pub fn admission_policy_64kib() -> AdmissionPolicy {
    AdmissionPolicy::Admission(AdmissionLimits {
        max_module_bytes: 64 * 1024,
        ..AdmissionLimits::default()
    })
}
