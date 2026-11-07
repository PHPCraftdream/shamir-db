use crate::tests::wasm::admission::support::{
    admit, limits_with, load, module, narrow, probe, wide,
};
use crate::AdmissionError;

const BATCH_PUT_IMPORT: &str =
    r#"(import "shamir_host" "batch_put" (func (param i32 i32 i32 i32)))"#;

/// `n` explicit function types with pairwise distinct signatures.
fn extra_types(n: usize) -> String {
    (0..n)
        .map(|i| format!("(type (func {}))", "(param i32)".repeat(i)))
        .collect()
}

/// One function exported under `n` extra names.
fn extra_exports(n: usize) -> String {
    let exports: String = (0..n)
        .map(|i| format!(r#"(export "e{i}" (func $f))"#))
        .collect();
    format!("(func $f) {exports}")
}

#[test]
fn module_bytes_at_limit_admitted() {
    let bytes = module("");
    let limits = limits_with(|l| l.max_module_bytes = bytes.len());
    let report = admit(&bytes, limits).unwrap();
    assert_eq!(report.byte_size, bytes.len());
}

#[test]
fn module_bytes_over_limit_rejected() {
    let bytes = module("");
    let k = bytes.len() - 1;
    let limits = limits_with(|l| l.max_module_bytes = k);
    let err = admit(&bytes, limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::ModuleTooLarge { limit, observed }
            if limit == wide(k) && observed == wide(k + 1)),
        "{err}"
    );
}

#[test]
fn functions_at_limit_admitted() {
    let k = narrow(probe().function_count) + 3;
    let limits = limits_with(|l| l.max_functions = k);
    let report = admit(&module(&"(func)".repeat(3)), limits).unwrap();
    assert_eq!(report.function_count, wide(k));
}

#[test]
fn functions_over_limit_rejected() {
    let k = narrow(probe().function_count) + 3;
    let limits = limits_with(|l| l.max_functions = k);
    let err = admit(&module(&"(func)".repeat(4)), limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::TooManyFunctions { limit, observed }
            if limit == wide(k) && observed == wide(k + 1)),
        "{err}"
    );
}

#[test]
fn types_at_limit_admitted() {
    let k = narrow(probe().type_count) + 3;
    let limits = limits_with(|l| l.max_types = k);
    let report = admit(&module(&extra_types(3)), limits).unwrap();
    assert_eq!(report.type_count, wide(k));
}

#[test]
fn types_over_limit_rejected() {
    let k = narrow(probe().type_count) + 3;
    let limits = limits_with(|l| l.max_types = k);
    let err = admit(&module(&extra_types(4)), limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::TooManyTypes { limit, observed }
            if limit == wide(k) && observed == wide(k + 1)),
        "{err}"
    );
}

#[test]
fn imports_at_limit_admitted() {
    let k = narrow(probe().import_count) + 2;
    let limits = limits_with(|l| l.max_imports = k);
    let report = admit(&module(&BATCH_PUT_IMPORT.repeat(2)), limits).unwrap();
    assert_eq!(report.import_count, wide(k));
}

#[test]
fn imports_over_limit_rejected() {
    let k = narrow(probe().import_count) + 2;
    let limits = limits_with(|l| l.max_imports = k);
    let err = admit(&module(&BATCH_PUT_IMPORT.repeat(3)), limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::TooManyImports { limit, observed }
            if limit == wide(k) && observed == wide(k + 1)),
        "{err}"
    );
}

#[test]
fn exports_at_limit_admitted() {
    let k = narrow(probe().export_count) + 2;
    let limits = limits_with(|l| l.max_exports = k);
    let report = admit(&module(&extra_exports(2)), limits).unwrap();
    assert_eq!(report.export_count, wide(k));
}

#[test]
fn exports_over_limit_rejected() {
    let k = narrow(probe().export_count) + 2;
    let limits = limits_with(|l| l.max_exports = k);
    let err = admit(&module(&extra_exports(3)), limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::TooManyExports { limit, observed }
            if limit == wide(k) && observed == wide(k + 1)),
        "{err}"
    );
}

#[test]
fn globals_at_limit_admitted() {
    let k = narrow(probe().global_count) + 2;
    let limits = limits_with(|l| l.max_globals = k);
    let report = admit(&module(&"(global i32 (i32.const 0))".repeat(2)), limits).unwrap();
    assert_eq!(report.global_count, wide(k));
}

#[test]
fn globals_over_limit_rejected() {
    let k = narrow(probe().global_count) + 2;
    let limits = limits_with(|l| l.max_globals = k);
    let err = admit(&module(&"(global i32 (i32.const 0))".repeat(3)), limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::TooManyGlobals { limit, observed }
            if limit == wide(k) && observed == wide(k + 1)),
        "{err}"
    );
}

#[test]
fn three_tables_rejected_under_admission_and_load() {
    let bytes = module(&"(table 1 funcref)".repeat(3));
    let admission = admit(&bytes, limits_with(|_| {})).unwrap_err();
    assert!(
        matches!(admission, AdmissionError::TooManyTables { observed: 3 }),
        "{admission}"
    );
    let loaded = load(&bytes).unwrap_err();
    assert!(
        matches!(loaded, AdmissionError::TooManyTables { observed: 3 }),
        "{loaded}"
    );
}
