use crate::registry::{ScalarError, SCALAR_ERROR_CODES};

#[test]
fn rv_funclib_24c30e50_builtin_catalog_covers_all_observed_codes() {
    let codes = [
        "arity",
        "unknown_function",
        "missing_arg",
        "type_mismatch",
        "out_of_range",
        "cast_failed",
        "unknown_type",
        "decode_failed",
        "encode_failed",
        "bad_pattern",
        "bad_regex",
        "bad_bounds",
        "domain",
        "div_by_zero",
        "empty",
        "no_match",
        "bad_pad",
        "missing_key",
        "bad_key",
        "bad_params",
        "bad_unit",
        "compute",
        "parse",
    ];
    assert_eq!(SCALAR_ERROR_CODES, codes);
    for code in codes {
        assert_eq!(ScalarError::builtin(code).code, code);
    }
    assert_eq!(
        ScalarError::new("external_custom_code").code,
        "external_custom_code"
    );
}

#[test]
fn rv_funclib_24c30e50_unknown_builtin_code_is_rejected() {
    assert!(std::panic::catch_unwind(|| ScalarError::builtin("not_in_builtin_catalog")).is_err());
    assert!(!SCALAR_ERROR_CODES.contains(&"not_in_builtin_catalog"));
}
