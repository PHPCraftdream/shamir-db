use crate::api::{CompileLimits, Stage};
use crate::back::emit::{emit_compiled_module, emit_runtime_module, EmitError};
use crate::back::ir::IrModule;
use crate::front::{self, CompileBudget};

use shamir_wasm_admission::{admit_module, AdmissionPolicy};

use wasmparser::{
    ExternalKind, FuncValidatorAllocations, Parser, Payload, ValidPayload, Validator, WasmFeatures,
    WasmFeaturesInflated,
};

/// Parses, checks, lowers, verifies and emits one fixture source.
fn compile(source: &str) -> Vec<u8> {
    compile_with(source, &CompileLimits::default()).expect("fixture compiles")
}

fn compile_with(source: &str, limits: &CompileLimits) -> Result<Vec<u8>, EmitError> {
    let mut budget = CompileBudget::new(limits).expect("configured limits form a compile budget");
    emit_compiled_module(&lowered_ir(source), limits, &mut budget)
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
    // Validate function by function so one run names every invalid function.
    let mut failures: Vec<(usize, String)> = Vec::new();
    let mut module_error = None;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.expect("emitted module parses");
        match validator.payload(&payload) {
            Ok(ValidPayload::Func(to_validate, body)) => {
                let mut function = to_validate.into_validator(FuncValidatorAllocations::default());
                if let Err(error) = function.validate(&body) {
                    failures.push((error.offset(), error.to_string()));
                }
            }
            Ok(_) => {}
            Err(error) => {
                module_error = Some(error.to_string());
                break;
            }
        }
    }
    if module_error.is_some() || !failures.is_empty() {
        let mut report = format!(
            "module-level error: {module_error:?}
"
        );
        for (position, (offset, message)) in failures.iter().enumerate() {
            report.push_str(&format!(
                "invalid function: {message}
"
            ));
            if position < 3 {
                report.push_str(&describe_failure(bytes, *offset));
            } else {
                report.push_str(&format!(
                    "(offset {offset})
"
                ));
            }
        }
        panic!(
            "emitted module validates:
{report}"
        );
    }
}

/// Names the function whose body contains the failing byte offset and lists
/// its operators with their byte offsets, so a validation failure points at
/// the emitted code.
fn describe_failure(bytes: &[u8], offset: usize) -> String {
    let mut imported = 0usize;
    let mut next_body = 0usize;
    let mut report = format!("offset {offset} is outside every function body");
    for payload in Parser::new(0).parse_all(bytes).flatten() {
        match payload {
            Payload::ImportSection(reader) => imported = reader.count() as usize,
            Payload::CodeSectionEntry(body) => {
                let range = body.range();
                if range.start <= offset && offset < range.end {
                    report = format!(
                        "function index {}, body bytes {}..{}, offset {offset}
",
                        imported + next_body,
                        range.start,
                        range.end
                    );
                    if let Ok(reader) = body.get_operators_reader() {
                        for item in reader.into_iter_with_offsets() {
                            match item {
                                Ok((operator, at)) => {
                                    let mark = if at == offset { ">>" } else { "  " };
                                    report.push_str(&format!(
                                        "{mark} {at}: {operator:?}
"
                                    ));
                                }
                                Err(error) => {
                                    report.push_str(&format!(
                                        "operator read error: {error}
"
                                    ));
                                    break;
                                }
                            }
                        }
                    }
                }
                next_body += 1;
            }
            _ => {}
        }
    }
    report
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
