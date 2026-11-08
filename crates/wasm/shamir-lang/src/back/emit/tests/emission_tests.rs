use crate::api::{CompileLimits, Stage};
use crate::back::emit::{emit_compiled_module, emit_runtime_module, EmitError};
use crate::back::ir::IrModule;
use crate::front::{self, CompileBudget};

use shamir_wasm_admission::{admit_module, AdmissionPolicy};

use wasmparser::{ExternalKind, Parser, Payload, Validator, WasmFeatures, WasmFeaturesInflated};

/// Parses, checks, lowers, verifies and emits one fixture source.
fn compile(source: &str) -> Vec<u8> {
    compile_with(source, &CompileLimits::default()).expect("fixture compiles")
}

fn compile_with(source: &str, limits: &CompileLimits) -> Result<Vec<u8>, EmitError> {
    emit_compiled_module(&lowered_ir(source), limits)
}

fn lowered_ir(source: &str) -> IrModule {
    let module = front::parse(source, &CompileLimits::default()).expect("fixture source parses");
    let typed =
        crate::sem::check(&module, &CompileLimits::default(), Stage::Mvp).expect("fixture checks");
    let mut budget =
        CompileBudget::new(&CompileLimits::default()).expect("default limits form a budget");
    crate::back::lower::lower(&module, &typed, &CompileLimits::default(), &mut budget)
        .expect("fixture lowers")
}

fn validate(bytes: &[u8]) {
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
        .validate_all(bytes)
        .expect("emitted module validates");
}

fn exports_of(bytes: &[u8]) -> Vec<(String, ExternalKind)> {
    let mut exports = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::ExportSection(reader) = payload.expect("parse") {
            for export in reader {
                let export = export.expect("export");
                exports.push((export.name.to_string(), export.kind));
            }
        }
    }
    exports
}

const SCALAR_EXAMPLE: &str = "fn add(a: i64, b: i64) -> i64 { return a + b; }\n\
     @entry scalar sum(a: i64, b: i64) -> i64 { return add(a, b); }\n";

const WHILE_BREAK: &str = "@entry scalar probe(c: bool) -> i64 {\n\
     let i: i64 = 0;\n\
     while (i < 10) {\n\
     i = i + 1;\n\
     if (c) { continue; }\n\
     if (i == 7) { break; }\n\
     }\n\
     return i;\n\
     }\n";

const RECORD_EXAMPLE: &str = "@entry scalar probe() -> i64 {\n\
     let r: { b: i64, a: i64 } = { b: 1, a: 2 };\n\
     return r.a + r.b;\n\
     }\n";

#[test]
fn scalar_example_compiles_and_validates() {
    let bytes = compile(SCALAR_EXAMPLE);
    validate(&bytes);
    admit_module(&bytes, &AdmissionPolicy::Load).expect("admitted");
}

#[test]
fn while_break_example_compiles_and_validates() {
    let bytes = compile(WHILE_BREAK);
    validate(&bytes);
    admit_module(&bytes, &AdmissionPolicy::Load).expect("admitted");
}

#[test]
fn record_example_compiles_and_validates() {
    let bytes = compile(RECORD_EXAMPLE);
    validate(&bytes);
    admit_module(&bytes, &AdmissionPolicy::Load).expect("admitted");
}

#[test]
fn emission_is_deterministic() {
    for source in [SCALAR_EXAMPLE, WHILE_BREAK, RECORD_EXAMPLE] {
        let first = compile(source);
        let second = compile(source);
        assert_eq!(first, second, "emission must be deterministic");
    }
}

#[test]
fn compiled_exports_exactly_the_guest_contract() {
    let bytes = compile(SCALAR_EXAMPLE);
    assert_eq!(
        exports_of(&bytes),
        vec![
            ("memory".to_string(), ExternalKind::Memory),
            ("shamir_alloc".to_string(), ExternalKind::Func),
            ("shamir_call".to_string(), ExternalKind::Func),
        ]
    );
}

#[test]
fn runtime_module_exports_are_unchanged() {
    let bytes = emit_runtime_module(&[], crate::back::runtime::RuntimeCaps::default())
        .expect("runtime module");
    assert_eq!(
        exports_of(&bytes),
        vec![
            ("memory".to_string(), ExternalKind::Memory),
            ("shamir_alloc".to_string(), ExternalKind::Func),
            ("shamir_call".to_string(), ExternalKind::Func),
        ]
    );
}

#[test]
fn module_rejects_output_over_limit() {
    let limits = CompileLimits {
        max_output_bytes: 1,
        ..CompileLimits::default()
    };
    let error = compile_with(SCALAR_EXAMPLE, &limits)
        .expect_err("a tiny output cap must reject the module");
    assert!(matches!(
        error,
        EmitError::LimitExceeded { code: "B001", .. }
    ));
}
