use crate::api::{CompileLimits, Stage};
use crate::back::emit::{emit_compiled_module, EmitError};
use crate::back::ir::IrModule;
use crate::front::{self, CompileBudget};

use shamir_wasm_admission::{admit_module, AdmissionLimits, AdmissionPolicy};

use wasmparser::{
    ExternalKind, FuncValidatorAllocations, Parser, Payload, ValidPayload, Validator, WasmFeatures,
    WasmFeaturesInflated,
};

/// Parses, checks, lowers and emits one fixture source.
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
        panic!(
            "emitted module validates: module-level error: {module_error:?}, failures: {failures:?}"
        );
    }
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

/// Collects the (module, name) pairs of the module's imports.
fn imports_of(bytes: &[u8]) -> Vec<(String, String)> {
    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::ImportSection(reader) = payload.expect("parse") {
            for group in reader {
                for import in group.expect("import group") {
                    let (_, import) = import.expect("import");
                    imports.push((import.module.to_string(), import.name.to_string()));
                }
            }
        }
    }
    imports
}

const VALIDATOR_EXAMPLE: &str = "@entry validator require_name(\n\
     record: { name: string }?,\n\
     old_record: { name: string }?\n\
     ) -> Validation {\n\
     if (is_null(record)) {\n\
     return record_error(\"record_required\");\n\
     }\n\
     let current: { name: string } = unwrap(record);\n\
     if (current.name == \"\") {\n\
     return reject(\"name\", \"name_empty\");\n\
     }\n\
     return accept();\n\
     }\n";

#[test]
fn validator_module_is_valid_and_self_admitted() {
    let bytes = compile(VALIDATOR_EXAMPLE);
    validate(&bytes);
    // Derivation: the validator module keeps the scalar module's ABI surface,
    // so the full admission gate (caps + contract check) must admit it.
    let report = admit_module(
        &bytes,
        &AdmissionPolicy::Admission(AdmissionLimits::default()),
    )
    .expect("validator module admitted");
    assert_eq!(report.byte_size, bytes.len());
}

#[test]
fn validator_emission_is_deterministic() {
    let first = compile(VALIDATOR_EXAMPLE);
    let second = compile(VALIDATOR_EXAMPLE);
    assert_eq!(first, second, "emission must be deterministic");
}

#[test]
fn validator_module_exports_the_three_exports() {
    let bytes = compile(VALIDATOR_EXAMPLE);
    // Derivation: same ABI surface as the scalar module (design section 7,
    // "Emitted module sketch"): exactly the three guest-contract exports.
    assert_eq!(
        exports_of(&bytes),
        vec![
            ("memory".to_string(), ExternalKind::Memory),
            ("shamir_alloc".to_string(), ExternalKind::Func),
            ("shamir_call".to_string(), ExternalKind::Func),
        ]
    );
    // Derivation: the single import is the raise_error host trap hook.
    assert_eq!(
        imports_of(&bytes),
        vec![("shamir_host".to_string(), "raise_error".to_string())]
    );
}

#[test]
fn validator_static_image_carries_the_three_descriptors() {
    let ir = lowered_ir(VALIDATOR_EXAMPLE);
    // Derivation: the synthetic descriptors bake the field names once each
    // into the static image: ["errors","stop"], ["code","field"], ["code"].
    assert!(!ir.static_bytes.is_empty());
    for needle in [b"errors".as_slice(), b"stop", b"code", b"field"] {
        assert!(
            ir.static_bytes
                .windows(needle.len())
                .any(|window| window == needle),
            "static image must contain {needle:?}"
        );
    }
}
