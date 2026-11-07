use crate::tests::support::{admit, assemble, limits_with, load, module};
use crate::{verify_wasm_module, AdmissionError};

#[test]
fn operator_cap_applies_to_admission_but_not_load() {
    let bytes = module("");
    assert!(load(&bytes).is_ok());
    let err = admit(&bytes, limits_with(|l| l.max_operators_per_function = 3)).unwrap_err();
    assert!(
        matches!(err, AdmissionError::TooManyOperators { .. }),
        "{err}"
    );
}

#[test]
fn contract_applies_to_admission_but_not_load() {
    let bytes = assemble("(module)");
    assert!(load(&bytes).is_ok());
    let err = admit(&bytes, limits_with(|_| {})).unwrap_err();
    assert!(
        matches!(err, AdmissionError::MissingExport { name: "memory" }),
        "{err}"
    );
}

#[test]
fn unsanctioned_import_rejected_under_both_policies() {
    let bytes = module(r#"(import "evil" "syscall" (func))"#);
    for err in [
        load(&bytes).unwrap_err(),
        admit(&bytes, limits_with(|_| {})).unwrap_err(),
    ] {
        assert!(
            matches!(&err, AdmissionError::UnsanctionedImport { module: m, name, .. }
                if m == "evil" && name == "syscall"),
            "{err}"
        );
    }
}

#[test]
fn component_encoding_rejected_under_both_policies() {
    let bytes = assemble("(component)");
    for err in [
        load(&bytes).unwrap_err(),
        admit(&bytes, limits_with(|_| {})).unwrap_err(),
    ] {
        assert!(matches!(err, AdmissionError::ComponentEncoding), "{err}");
    }
}

#[test]
fn garbage_bytes_rejected_under_both_policies() {
    let bytes = b"definitely not a wasm module";
    for err in [
        load(bytes).unwrap_err(),
        admit(bytes, limits_with(|_| {})).unwrap_err(),
    ] {
        assert!(
            matches!(
                err,
                AdmissionError::Parse(_) | AdmissionError::Validation(_)
            ),
            "{err}"
        );
    }
}

#[test]
fn empty_input_rejected_under_both_policies() {
    for err in [
        load(&[]).unwrap_err(),
        admit(&[], limits_with(|_| {})).unwrap_err(),
    ] {
        assert!(
            matches!(
                err,
                AdmissionError::Parse(_) | AdmissionError::Validation(_)
            ),
            "{err}"
        );
    }
}

#[test]
fn verify_wasm_module_still_accepts_sanctioned_module() {
    let bytes = module(r#"(import "shamir_host" "batch_put" (func (param i32 i32 i32 i32)))"#);
    assert!(verify_wasm_module(&bytes).is_ok());
}
