use crate::validator::{decode_validation_result, ValidationError, ValidatorDecodeError};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

/// Helper: build a `QueryValue::Map` from key-value pairs.
fn qmap(pairs: Vec<(&str, QueryValue)>) -> QueryValue {
    let mut m = new_map();
    for (k, v) in pairs {
        m.insert(k.to_owned(), v);
    }
    QueryValue::Map(m)
}

// ----- null => valid --------------------------------------------------------

#[test]
fn null_is_valid() {
    let out = decode_validation_result(&QueryValue::Null).unwrap();
    assert!(out.errors.is_empty());
    assert!(!out.stop);
}

// ----- empty list => valid --------------------------------------------------

#[test]
fn empty_list_is_valid() {
    let out = decode_validation_result(&QueryValue::List(vec![])).unwrap();
    assert!(out.errors.is_empty());
    assert!(!out.stop);
}

// ----- list of map errors ---------------------------------------------------

#[test]
fn list_of_map_errors() {
    let items = QueryValue::List(vec![
        qmap(vec![
            (
                "field",
                QueryValue::List(vec![
                    QueryValue::Str("address".into()),
                    QueryValue::Str("zip".into()),
                ]),
            ),
            ("code", QueryValue::Str("invalid_zip".into())),
        ]),
        qmap(vec![("code", QueryValue::Str("missing_name".into()))]),
    ]);

    let out = decode_validation_result(&items).unwrap();
    assert_eq!(out.errors.len(), 2);
    assert!(!out.stop);

    assert_eq!(
        out.errors[0],
        ValidationError {
            field: Some(vec!["address".into(), "zip".into()]),
            code: "invalid_zip".into(),
        }
    );
    assert_eq!(
        out.errors[1],
        ValidationError {
            field: None,
            code: "missing_name".into(),
        }
    );
}

// ----- list with a bare-string error ----------------------------------------

#[test]
fn list_with_bare_string_error() {
    let items = QueryValue::List(vec![
        QueryValue::Str("too_short".into()),
        qmap(vec![
            (
                "field",
                QueryValue::List(vec![QueryValue::Str("email".into())]),
            ),
            ("code", QueryValue::Str("invalid_format".into())),
        ]),
    ]);

    let out = decode_validation_result(&items).unwrap();
    assert_eq!(out.errors.len(), 2);
    assert_eq!(
        out.errors[0],
        ValidationError {
            field: None,
            code: "too_short".into(),
        }
    );
    assert_eq!(
        out.errors[1],
        ValidationError {
            field: Some(vec!["email".into()]),
            code: "invalid_format".into(),
        }
    );
}

// ----- map { errors, stop: true } -------------------------------------------

#[test]
fn map_with_errors_and_stop_true() {
    let v = qmap(vec![
        (
            "errors",
            QueryValue::List(vec![QueryValue::Str("fatal".into())]),
        ),
        ("stop", QueryValue::Bool(true)),
    ]);

    let out = decode_validation_result(&v).unwrap();
    assert_eq!(out.errors.len(), 1);
    assert_eq!(out.errors[0].code, "fatal");
    assert!(out.stop);
}

// ----- map { errors } (stop defaults false) ---------------------------------

#[test]
fn map_with_errors_stop_defaults_false() {
    let v = qmap(vec![(
        "errors",
        QueryValue::List(vec![QueryValue::Str("warn".into())]),
    )]);

    let out = decode_validation_result(&v).unwrap();
    assert_eq!(out.errors.len(), 1);
    assert!(!out.stop);
}

// ----- map with field = null means record-level -----------------------------

#[test]
fn map_error_with_field_null() {
    let item = qmap(vec![
        ("field", QueryValue::Null),
        ("code", QueryValue::Str("record_level".into())),
    ]);
    let out = decode_validation_result(&QueryValue::List(vec![item])).unwrap();
    assert_eq!(out.errors[0].field, None);
}

// ----- malformed: missing code in map item ----------------------------------

#[test]
fn malformed_missing_code() {
    let item = qmap(vec![(
        "field",
        QueryValue::List(vec![QueryValue::Str("x".into())]),
    )]);
    let v = QueryValue::List(vec![item]);

    let err = decode_validation_result(&v).unwrap_err();
    assert!(
        matches!(err, ValidatorDecodeError::MissingCode),
        "expected MissingCode, got: {err}"
    );
}

// ----- malformed: non-string code -------------------------------------------

#[test]
fn malformed_non_string_code() {
    let item = qmap(vec![("code", QueryValue::Int(42))]);
    let v = QueryValue::List(vec![item]);

    let err = decode_validation_result(&v).unwrap_err();
    assert!(
        matches!(err, ValidatorDecodeError::NonStringCode),
        "expected NonStringCode, got: {err}"
    );
}

// ----- malformed: wrong root type -------------------------------------------

#[test]
fn malformed_wrong_root_type() {
    let err = decode_validation_result(&QueryValue::Int(1)).unwrap_err();
    assert!(
        matches!(err, ValidatorDecodeError::UnexpectedRootType),
        "expected UnexpectedRootType, got: {err}"
    );
}

#[test]
fn malformed_root_string() {
    let err = decode_validation_result(&QueryValue::Str("nope".into())).unwrap_err();
    assert!(matches!(err, ValidatorDecodeError::UnexpectedRootType));
}

// ----- malformed: bad item type (not string or map) -------------------------

#[test]
fn malformed_bad_item_type() {
    let v = QueryValue::List(vec![QueryValue::Bool(true)]);
    let err = decode_validation_result(&v).unwrap_err();
    assert!(
        matches!(err, ValidatorDecodeError::BadItemType),
        "expected BadItemType, got: {err}"
    );
}

// ----- malformed: map without "errors" key ----------------------------------

#[test]
fn malformed_map_without_errors_key() {
    let v = qmap(vec![("stop", QueryValue::Bool(true))]);
    let err = decode_validation_result(&v).unwrap_err();
    assert!(matches!(err, ValidatorDecodeError::UnexpectedRootType));
}
