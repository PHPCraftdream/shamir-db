use crate::tests::support::{admit, assemble, limits_with};
use crate::AdmissionError;

const CHECKED_OK: &str =
    r#"(import "shamir_host" "call_checked" (func (param i32 i32 i32 i32 i32 i32) (result i64)))"#;
const CHECKED_BAD: &str =
    r#"(import "shamir_host" "call_checked" (func (param i32 i32 i32 i32 i32) (result i64)))"#;
const MEMORY: &str = r#"(memory (export "memory") 1)"#;
const ALLOC: &str = r#"(func (export "shamir_alloc") (param i32) (result i32) (i32.const 0))"#;
const CALL: &str = r#"(func (export "shamir_call") (param i32 i32) (result i64) (i64.const 0))"#;

/// Admission error of a module made of `items`.
fn rejection(items: &[&str]) -> AdmissionError {
    let bytes = assemble(&format!("(module {})", items.join(" ")));
    admit(&bytes, limits_with(|_| {})).unwrap_err()
}

#[test]
fn checked_call_exact_signature_admitted() {
    let bytes = assemble(&format!("(module {CHECKED_OK} {MEMORY} {ALLOC} {CALL})"));
    assert!(admit(&bytes, limits_with(|_| {})).is_ok());
}

#[test]
fn checked_call_wrong_signature_rejected() {
    let bytes = assemble(&format!("(module {CHECKED_BAD} {MEMORY} {ALLOC} {CALL})"));
    assert!(matches!(
        admit(&bytes, limits_with(|_| {})),
        Err(AdmissionError::WrongImportType {
            name: "call_checked",
            ..
        })
    ));
}

#[test]
fn missing_memory_export_rejected() {
    let err = rejection(&[ALLOC, CALL]);
    assert!(
        matches!(err, AdmissionError::MissingExport { name: "memory" }),
        "{err}"
    );
}

#[test]
fn missing_alloc_export_rejected() {
    let err = rejection(&[MEMORY, CALL]);
    assert!(
        matches!(
            err,
            AdmissionError::MissingExport {
                name: "shamir_alloc"
            }
        ),
        "{err}"
    );
}

#[test]
fn missing_call_export_rejected() {
    let err = rejection(&[MEMORY, ALLOC]);
    assert!(
        matches!(
            err,
            AdmissionError::MissingExport {
                name: "shamir_call"
            }
        ),
        "{err}"
    );
}

#[test]
fn alloc_with_wrong_parameter_type_rejected() {
    let alloc = r#"(func (export "shamir_alloc") (param i64) (result i32) (i32.const 0))"#;
    let err = rejection(&[MEMORY, alloc, CALL]);
    assert!(
        matches!(&err, AdmissionError::WrongExportType { name: "shamir_alloc", found, .. } if found == "(i64) -> i32"),
        "{err}"
    );
}

#[test]
fn call_with_missing_parameter_rejected() {
    let call = r#"(func (export "shamir_call") (param i32) (result i64) (i64.const 0))"#;
    let err = rejection(&[MEMORY, ALLOC, call]);
    assert!(
        matches!(&err, AdmissionError::WrongExportType { name: "shamir_call", found, .. } if found == "(i32) -> i64"),
        "{err}"
    );
}

#[test]
fn call_with_wrong_result_type_rejected() {
    let call = r#"(func (export "shamir_call") (param i32 i32) (result i32) (i32.const 0))"#;
    let err = rejection(&[MEMORY, ALLOC, call]);
    assert!(
        matches!(&err, AdmissionError::WrongExportType { name: "shamir_call", found, .. } if found == "(i32, i32) -> i32"),
        "{err}"
    );
}

#[test]
fn memory_exported_as_global_rejected() {
    let global = r#"(global (export "memory") i32 (i32.const 0))"#;
    let err = rejection(&[global, ALLOC, CALL]);
    assert!(
        matches!(&err, AdmissionError::WrongExportKind { name: "memory", found, .. } if found == "global"),
        "{err}"
    );
}

#[test]
fn memory_exported_as_table_rejected() {
    let table = r#"(table (export "memory") 1 funcref)"#;
    let err = rejection(&[table, ALLOC, CALL]);
    assert!(
        matches!(&err, AdmissionError::WrongExportKind { name: "memory", found, .. } if found == "table"),
        "{err}"
    );
}

#[test]
fn call_exported_as_global_rejected() {
    let global = r#"(global (export "shamir_call") i32 (i32.const 0))"#;
    let err = rejection(&[MEMORY, ALLOC, global]);
    assert!(
        matches!(&err, AdmissionError::WrongExportKind { name: "shamir_call", found, .. } if found == "global"),
        "{err}"
    );
}
