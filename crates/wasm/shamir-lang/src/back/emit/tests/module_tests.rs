use crate::back::emit::emit_runtime_module;
use crate::back::runtime::{RuntimeCaps, RuntimeError, MAX_STATIC_BYTES};

use shamir_wasm_admission::{admit_module, AdmissionLimits, AdmissionPolicy};

use wasmparser::{ExternalKind, Parser, Payload, Validator, WasmFeatures, WasmFeaturesInflated};

fn admission_policy() -> AdmissionPolicy {
    AdmissionPolicy::Admission(AdmissionLimits {
        max_module_bytes: 64 * 1024,
        ..AdmissionLimits::default()
    })
}

#[test]
fn module_passes_default_admission() {
    let bytes = emit_runtime_module(&[], RuntimeCaps::default()).expect("module emission");
    let report = admit_module(&bytes, &admission_policy()).expect("admitted module");
    assert_eq!(report.abi_version, 1);
}

/// Validator with the exact admitted feature set, copied field-for-field from
/// `shamir_wasm_admission::scan::features` so this module is held to the same
/// feature envelope the admission scanner enforces.
#[test]
fn module_validates_under_the_admitted_feature_set() {
    let bytes = emit_runtime_module(&[], RuntimeCaps::default()).expect("module emission");
    let mut validator =
        Validator::new_with_features(WasmFeatures::from_inflated(WasmFeaturesInflated {
            mutable_global: true,
            saturating_float_to_int: true,
            sign_extension: true,
            reference_types: true,
            multi_value: true,
            bulk_memory: true,
            simd: false,
            relaxed_simd: false,
            threads: false,
            shared_everything_threads: false,
            tail_call: false,
            floats: true,
            multi_memory: false,
            exceptions: false,
            memory64: false,
            extended_const: false,
            component_model: false,
            function_references: false,
            memory_control: false,
            gc: false,
            custom_page_sizes: false,
            legacy_exceptions: false,
            gc_types: false,
            stack_switching: false,
            wide_arithmetic: false,
            cm_values: false,
            cm_nested_names: false,
            cm_async: false,
            cm_async_stackful: false,
            cm_more_async_builtins: false,
            cm_threading: false,
            cm_error_context: false,
            cm_fixed_length_lists: false,
            cm_gc: false,
            // Sub-bits of reference types / bulk memory: `from_inflated` sets
            // each bit independently, so they must be on for the full
            // proposals.
            call_indirect_overlong: true,
            bulk_memory_opt: true,
            custom_descriptors: false,
            compact_imports: false,
            cm_map: false,
            cm64: false,
            cm_implements: false,
            cm_canon_names: false,
        }));
    validator
        .validate_all(&bytes)
        .expect("emitted module validates");
}

#[test]
fn module_exports_exactly_the_guest_contract() {
    let bytes = emit_runtime_module(&[], RuntimeCaps::default()).expect("module emission");
    let mut exports = Vec::new();
    for payload in Parser::new(0).parse_all(&bytes) {
        if let Payload::ExportSection(reader) = payload.expect("parse") {
            for export in reader {
                let export = export.expect("export");
                exports.push((export.name.to_string(), export.kind));
            }
        }
    }
    assert_eq!(
        exports,
        vec![
            ("memory".to_string(), ExternalKind::Memory),
            ("shamir_alloc".to_string(), ExternalKind::Func),
            ("shamir_call".to_string(), ExternalKind::Func),
        ]
    );
}

#[test]
fn module_emission_is_deterministic() {
    let first = emit_runtime_module(&[], RuntimeCaps::default()).expect("first emission");
    let second = emit_runtime_module(&[], RuntimeCaps::default()).expect("second emission");
    assert_eq!(first, second);
}

#[test]
fn module_rejects_oversized_static_bytes() {
    let oversized = vec![0_u8; usize::try_from(MAX_STATIC_BYTES).expect("cap fits usize") + 1];
    let error = emit_runtime_module(&oversized, RuntimeCaps::default())
        .expect_err("static above the cap is rejected");
    assert!(matches!(
        error,
        RuntimeError::RuntimeTooLarge { limit, observed }
            if limit == MAX_STATIC_BYTES && observed == MAX_STATIC_BYTES + 1
    ));
}

#[test]
fn module_rejects_caps_above_the_design_ceiling() {
    let caps = RuntimeCaps {
        cells: u64::MAX,
        ..RuntimeCaps::default()
    };
    let error = emit_runtime_module(&[], caps).expect_err("exhaustive caps are rejected");
    assert_eq!(error, RuntimeError::CapAboveCeiling);
}
