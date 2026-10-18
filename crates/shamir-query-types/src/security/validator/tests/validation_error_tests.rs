use shamir_types::types::value::QueryValue;

use crate::validator::ValidationError;

#[test]
fn serde_with_field() {
    let err = ValidationError {
        field: Some(vec!["address".into(), "zip".into()]),
        code: "invalid_zip".into(),
    };
    let bytes = rmp_serde::to_vec_named(&err).unwrap();
    let decoded: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    // field must be a list ["address", "zip"]
    let field_val = decoded.get("field").expect("field key present");
    assert!(matches!(field_val, QueryValue::List(_)));
    if let QueryValue::List(items) = field_val {
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str(), Some("address"));
        assert_eq!(items[1].as_str(), Some("zip"));
    }
    // code must be "invalid_zip"
    assert_eq!(
        decoded.get("code").and_then(QueryValue::as_str),
        Some("invalid_zip")
    );

    let back: ValidationError = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, err);
}

#[test]
fn serde_without_field() {
    let err = ValidationError {
        field: None,
        code: "at_least_one_contact".into(),
    };
    let bytes = rmp_serde::to_vec_named(&err).unwrap();
    let decoded: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    // `field` must be absent, not null.
    assert!(decoded.get("field").is_none());
    assert_eq!(
        decoded.get("code").and_then(QueryValue::as_str),
        Some("at_least_one_contact")
    );

    let back: ValidationError = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, err);
}

#[test]
fn serde_round_trip_msgpack() {
    let err = ValidationError {
        field: Some(vec!["name".into()]),
        code: "too_short".into(),
    };
    let bytes = rmp_serde::to_vec_named(&err).unwrap();
    let back: ValidationError = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, err);
}
