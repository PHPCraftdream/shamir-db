use crate::validator::{
    decode_validation_result, validation_to_query_value, Validation, ValidationError,
};

/// `decode(validation_to_query_value(v))` must round-trip the errors and
/// the stop flag for every variant the host encoder can produce.
fn assert_round_trip(v: &Validation) {
    let encoded = validation_to_query_value(v);
    let decoded = decode_validation_result(&encoded).unwrap();
    assert_eq!(decoded.errors, v.errors, "errors mismatch");
    assert_eq!(decoded.stop, v.stop, "stop mismatch");
}

#[test]
fn round_trip_accept() {
    let v = Validation::accept();
    assert!(v.is_ok());
    assert_round_trip(&v);
}

#[test]
fn round_trip_single_error_record_level() {
    let v = Validation::reject("invalid");
    assert_eq!(v.errors.len(), 1);
    assert_round_trip(&v);
}

#[test]
fn round_trip_single_error_field_bound() {
    let mut v = Validation::accept();
    v.field_error(vec!["email".into()], "bad_format");
    assert_round_trip(&v);
}

#[test]
fn round_trip_multi_error_with_stop() {
    let mut v = Validation::accept();
    v.field_error(vec!["address".into(), "zip".into()], "invalid_zip")
        .error("missing_name")
        .stop();
    assert_eq!(v.errors.len(), 2);
    assert!(v.stop);
    assert_round_trip(&v);

    // Also verify the concrete error values.
    let encoded = validation_to_query_value(&v);
    let decoded = decode_validation_result(&encoded).unwrap();
    assert_eq!(
        decoded.errors[0],
        ValidationError {
            field: Some(vec!["address".into(), "zip".into()]),
            code: "invalid_zip".into(),
        }
    );
    assert_eq!(
        decoded.errors[1],
        ValidationError {
            field: None,
            code: "missing_name".into(),
        }
    );
}
