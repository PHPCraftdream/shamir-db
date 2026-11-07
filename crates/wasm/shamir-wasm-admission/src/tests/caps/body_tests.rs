use crate::tests::support::{
    admit, limits_with, module, module_with_call_body, narrow, probe, wide,
};
use crate::AdmissionError;

fn extra_locals(n: usize) -> String {
    format!("(func (local {}))", "i32 ".repeat(n))
}

/// `n` nested empty blocks.
fn nested_blocks(n: usize) -> String {
    format!("{}{}", "(block ".repeat(n), ")".repeat(n))
}

/// `br_table` with `labels` labels in total (the last one is the default).
fn br_table_body(labels: usize) -> String {
    format!("(block (br_table {} (i32.const 0)))", "0 ".repeat(labels))
}

/// Function whose type has 2 params and `results` results.
fn func_with_results(results: usize) -> String {
    format!(
        "(func (param i32 i32) (result {}) {})",
        "i32 ".repeat(results),
        "(i32.const 0) ".repeat(results)
    )
}

#[test]
fn locals_at_limit_admitted() {
    let limits = limits_with(|l| l.max_locals_per_function = 3);
    assert!(admit(&module(&extra_locals(3)), limits).is_ok());
}

#[test]
fn locals_over_limit_rejected() {
    let limits = limits_with(|l| l.max_locals_per_function = 3);
    let err = admit(&module(&extra_locals(4)), limits).unwrap_err();
    assert!(
        matches!(
            err,
            AdmissionError::TooManyLocals {
                limit: 3,
                observed: 4,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn function_body_bytes_at_limit_admitted() {
    let largest = probe().largest_function_body_bytes;
    let limits = limits_with(|l| l.max_function_body_bytes = largest);
    let report = admit(&module(""), limits).unwrap();
    assert_eq!(report.largest_function_body_bytes, largest);
}

#[test]
fn function_body_bytes_over_limit_rejected() {
    let largest = probe().largest_function_body_bytes;
    let limits = limits_with(|l| l.max_function_body_bytes = largest - 1);
    let err = admit(&module(""), limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::FunctionBodyTooLarge { limit, observed, .. }
            if limit == wide(largest - 1) && observed == wide(largest)),
        "{err}"
    );
}

#[test]
fn operators_at_limit_admitted() {
    let operators = probe().max_operators_in_function;
    let limits = limits_with(|l| l.max_operators_per_function = narrow(operators));
    let report = admit(&module(""), limits).unwrap();
    assert_eq!(report.max_operators_in_function, operators);
}

#[test]
fn operators_over_limit_rejected() {
    let operators = probe().max_operators_in_function;
    let limits = limits_with(|l| l.max_operators_per_function = narrow(operators) - 1);
    let err = admit(&module(""), limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::TooManyOperators { limit, observed, .. }
            if limit == operators - 1 && observed == operators),
        "{err}"
    );
}

#[test]
fn block_depth_at_limit_admitted() {
    let limits = limits_with(|l| l.max_block_depth = 3);
    let report = admit(&module_with_call_body("", &nested_blocks(3)), limits).unwrap();
    assert_eq!(report.max_block_depth_seen, 3);
}

#[test]
fn block_depth_over_limit_rejected() {
    let limits = limits_with(|l| l.max_block_depth = 3);
    let err = admit(&module_with_call_body("", &nested_blocks(4)), limits).unwrap_err();
    assert!(
        matches!(
            err,
            AdmissionError::BlockDepthExceeded {
                limit: 3,
                observed: 4,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn br_table_targets_at_limit_admitted() {
    // Three labels = two targets plus the default label.
    let limits = limits_with(|l| l.max_br_table_targets = 2);
    assert!(admit(&module_with_call_body("", &br_table_body(3)), limits).is_ok());
}

#[test]
fn br_table_targets_over_limit_rejected() {
    // Four labels = three targets plus the default label.
    let limits = limits_with(|l| l.max_br_table_targets = 2);
    let err = admit(&module_with_call_body("", &br_table_body(4)), limits).unwrap_err();
    assert!(
        matches!(
            err,
            AdmissionError::BrTableTooLarge {
                limit: 2,
                observed: 3,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn params_and_results_at_limit_admitted() {
    let limits = limits_with(|l| l.max_params_and_results = 5);
    assert!(admit(&module(&func_with_results(3)), limits).is_ok());
}

#[test]
fn params_and_results_over_limit_rejected() {
    let limits = limits_with(|l| l.max_params_and_results = 5);
    let err = admit(&module(&func_with_results(4)), limits).unwrap_err();
    assert!(
        matches!(
            err,
            AdmissionError::TooManyParamsAndResults {
                limit: 5,
                observed: 6,
                ..
            }
        ),
        "{err}"
    );
}
