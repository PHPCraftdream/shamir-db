use crate::validator::ValidationError;

#[test]
fn serde_with_field() {
    let err = ValidationError {
        field: Some(vec!["address".into(), "zip".into()]),
        code: "invalid_zip".into(),
    };
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "field": ["address", "zip"],
            "code": "invalid_zip"
        })
    );
    let back: ValidationError = serde_json::from_value(json).unwrap();
    assert_eq!(back, err);
}

#[test]
fn serde_without_field() {
    let err = ValidationError {
        field: None,
        code: "at_least_one_contact".into(),
    };
    let json = serde_json::to_value(&err).unwrap();
    // `field` must be absent, not `null`.
    assert!(json.get("field").is_none());
    assert_eq!(json["code"], "at_least_one_contact");

    let back: ValidationError = serde_json::from_value(json).unwrap();
    assert_eq!(back, err);
}

#[test]
fn serde_round_trip_json_string() {
    let err = ValidationError {
        field: Some(vec!["name".into()]),
        code: "too_short".into(),
    };
    let s = serde_json::to_string(&err).unwrap();
    let back: ValidationError = serde_json::from_str(&s).unwrap();
    assert_eq!(back, err);
}
