use wasmparser::{Validator, WasmFeatures, WasmFeaturesInflated};

/// Validator with the exact admitted feature set; every flag is listed so a
/// new `wasmparser` default can never leak in.
///
/// Allowed: MVP + mutable globals, sign extension, saturating float-to-int,
/// multi-value, bulk memory, reference types (funcref only: `gc_types` off
/// rejects `externref`; the operator scan limits table use) and floats.
pub(crate) fn admitted_validator() -> Validator {
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
        // Sub-bits of reference types / bulk memory: `from_inflated` sets each
        // bit independently, so they must be on for the full proposals.
        call_indirect_overlong: true,
        bulk_memory_opt: true,
        custom_descriptors: false,
        compact_imports: false,
        cm_map: false,
        cm64: false,
        cm_implements: false,
        cm_canon_names: false,
    }))
}
