//! Tests for `Validation::into_value()` — verifies the exact `Value` shape
//! matches what `decode_validation_result` in `shamir-engine` expects.
//!
//! NOTE: A direct round-trip through the engine's decoder requires
//! `shamir_types::QueryValue` + `shamir_query_types::filter::FieldPath`,
//! which are host-side types. We verify the msgpack-level shape here;
//! the real cross-crate round-trip is covered in S5 (e2e tests).

use crate::validation::{Validation, ValidationError};
use crate::value::Value;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the "errors" list from a Validation `Value`.
fn extract_errors(v: &Value) -> &Vec<Value> {
    match v {
        Value::Map(entries) => {
            let errors_entry = entries.iter().find(|(k, _)| k == "errors").unwrap();
            match &errors_entry.1 {
                Value::List(list) => list,
                other => panic!("expected List for errors, got: {other:?}"),
            }
        }
        other => panic!("expected Map, got: {other:?}"),
    }
}

/// Extract the "stop" bool from a Validation `Value`.
fn extract_stop(v: &Value) -> bool {
    match v {
        Value::Map(entries) => {
            let stop_entry = entries.iter().find(|(k, _)| k == "stop").unwrap();
            match &stop_entry.1 {
                Value::Bool(b) => *b,
                other => panic!("expected Bool for stop, got: {other:?}"),
            }
        }
        other => panic!("expected Map, got: {other:?}"),
    }
}

/// Look up a string key in a Value::Map.
fn map_get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn accept_produces_empty_errors_stop_false() {
    let v = Validation::accept().into_value();
    assert!(extract_errors(&v).is_empty());
    assert!(!extract_stop(&v));
}

#[test]
fn reject_single_field_error() {
    let v = Validation::reject(["address", "zip"], "invalid_zip").into_value();
    let errors = extract_errors(&v);
    assert_eq!(errors.len(), 1);
    assert!(!extract_stop(&v));

    // Check the error map shape
    let err = &errors[0];
    let field = map_get(err, "field").unwrap();
    assert_eq!(
        *field,
        Value::List(vec![Value::Str("address".into()), Value::Str("zip".into()),])
    );
    let code = map_get(err, "code").unwrap();
    assert_eq!(*code, Value::Str("invalid_zip".into()));
}

#[test]
fn reject_single_segment_field() {
    let v = Validation::reject("email", "invalid_format").into_value();
    let errors = extract_errors(&v);
    assert_eq!(errors.len(), 1);

    let err = &errors[0];
    let field = map_get(err, "field").unwrap();
    assert_eq!(*field, Value::List(vec![Value::Str("email".into())]));
}

#[test]
fn record_error_omits_field_key() {
    let v = Validation::record_error("at_least_one_contact").into_value();
    let errors = extract_errors(&v);
    assert_eq!(errors.len(), 1);

    let err = &errors[0];
    // field key must be absent (not null) for record-level errors
    assert!(
        map_get(err, "field").is_none(),
        "record-level error must not have a 'field' key"
    );
    let code = map_get(err, "code").unwrap();
    assert_eq!(*code, Value::Str("at_least_one_contact".into()));
}

#[test]
fn chained_errors() {
    let v = Validation::reject("name", "too_short")
        .error(["address", "zip"], "invalid_zip")
        .record("missing_contact")
        .into_value();

    let errors = extract_errors(&v);
    assert_eq!(errors.len(), 3);
    assert!(!extract_stop(&v));

    // First: field=["name"], code="too_short"
    assert_eq!(
        *map_get(&errors[0], "field").unwrap(),
        Value::List(vec![Value::Str("name".into())])
    );
    assert_eq!(
        *map_get(&errors[0], "code").unwrap(),
        Value::Str("too_short".into())
    );

    // Second: field=["address","zip"], code="invalid_zip"
    assert_eq!(
        *map_get(&errors[1], "field").unwrap(),
        Value::List(vec![Value::Str("address".into()), Value::Str("zip".into()),])
    );

    // Third: record-level (no field key)
    assert!(map_get(&errors[2], "field").is_none());
    assert_eq!(
        *map_get(&errors[2], "code").unwrap(),
        Value::Str("missing_contact".into())
    );
}

#[test]
fn stop_sets_stop_true() {
    let v = Validation::reject("name", "too_short").stop().into_value();
    assert!(extract_stop(&v));
    assert_eq!(extract_errors(&v).len(), 1);
}

#[test]
fn accept_is_empty() {
    assert!(Validation::accept().is_empty());
}

#[test]
fn reject_is_not_empty() {
    assert!(!Validation::reject("x", "y").is_empty());
}

#[test]
fn from_vec_validation_error() {
    let errors = vec![
        ValidationError {
            field: Some(vec!["a".into()]),
            code: "c1".into(),
        },
        ValidationError {
            field: None,
            code: "c2".into(),
        },
    ];
    let validation: Validation = errors.into();
    assert!(!validation.is_empty());

    let v = validation.into_value();
    assert!(!extract_stop(&v));
    assert_eq!(extract_errors(&v).len(), 2);
}

#[test]
fn into_value_shape_matches_engine_decoder_contract() {
    // Verify the exact structure: a Map with keys "errors" (List) and
    // "stop" (Bool). Each error is a Map with "code" (Str) and
    // optionally "field" (List<Str>).
    let v = Validation::reject(["a", "b"], "bad")
        .record("oops")
        .stop()
        .into_value();

    // Root must be Map
    let entries = match &v {
        Value::Map(e) => e,
        other => panic!("expected Map, got: {other:?}"),
    };
    // Must have exactly 2 keys: "errors" and "stop"
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].0, "errors");
    assert_eq!(entries[1].0, "stop");
    assert_eq!(entries[1].1, Value::Bool(true));

    let errors = match &entries[0].1 {
        Value::List(l) => l,
        other => panic!("expected List, got: {other:?}"),
    };
    assert_eq!(errors.len(), 2);

    // First error: field-bound
    let e0 = match &errors[0] {
        Value::Map(e) => e,
        other => panic!("expected Map, got: {other:?}"),
    };
    assert_eq!(e0.len(), 2); // field + code
    assert_eq!(e0[0].0, "field");
    assert_eq!(
        e0[0].1,
        Value::List(vec![Value::Str("a".into()), Value::Str("b".into())])
    );
    assert_eq!(e0[1].0, "code");
    assert_eq!(e0[1].1, Value::Str("bad".into()));

    // Second error: record-level (no field key)
    let e1 = match &errors[1] {
        Value::Map(e) => e,
        other => panic!("expected Map, got: {other:?}"),
    };
    assert_eq!(e1.len(), 1); // code only
    assert_eq!(e1[0].0, "code");
    assert_eq!(e1[0].1, Value::Str("oops".into()));
}

/// Cross-check: encode `into_value()` to msgpack, decode with
/// `shamir_types::QueryValue`, and verify the engine's decoder would
/// accept it with the expected outcome.
///
/// This test uses the host-side `shamir_types` (available as a
/// dev-dependency) to confirm wire compatibility.
#[test]
fn msgpack_roundtrip_matches_host_query_value() {
    use shamir_types::types::value::QueryValue;

    let validation = Validation::reject(["address", "zip"], "invalid_zip")
        .record("missing_contact")
        .stop();
    let guest_value = validation.into_value();
    let bytes = crate::__rt::encode_value(&guest_value);

    // Decode as the host QueryValue
    let host_value: QueryValue = rmp_serde::from_slice(&bytes).unwrap();

    // Verify root structure
    match &host_value {
        QueryValue::Map(m) => {
            // "errors" key exists and is a list
            let errors = m.get("errors").unwrap();
            match errors {
                QueryValue::List(items) => {
                    assert_eq!(items.len(), 2);

                    // First error: field-bound
                    match &items[0] {
                        QueryValue::Map(e) => {
                            match e.get("field").unwrap() {
                                QueryValue::List(parts) => {
                                    assert_eq!(parts.len(), 2);
                                    assert_eq!(parts[0], QueryValue::Str("address".into()));
                                    assert_eq!(parts[1], QueryValue::Str("zip".into()));
                                }
                                other => panic!("expected List for field, got: {other:?}"),
                            }
                            assert_eq!(
                                *e.get("code").unwrap(),
                                QueryValue::Str("invalid_zip".into())
                            );
                        }
                        other => panic!("expected Map, got: {other:?}"),
                    }

                    // Second error: record-level (no "field" key)
                    match &items[1] {
                        QueryValue::Map(e) => {
                            assert!(
                                e.get("field").is_none(),
                                "record-level error must not have a 'field' key"
                            );
                            assert_eq!(
                                *e.get("code").unwrap(),
                                QueryValue::Str("missing_contact".into())
                            );
                        }
                        other => panic!("expected Map, got: {other:?}"),
                    }
                }
                other => panic!("expected List for errors, got: {other:?}"),
            }

            // "stop" key is true
            assert_eq!(*m.get("stop").unwrap(), QueryValue::Bool(true));
        }
        other => panic!("expected Map at root, got: {other:?}"),
    }
}

#[test]
fn accept_msgpack_roundtrip_matches_host() {
    use shamir_types::types::value::QueryValue;

    let guest_value = Validation::accept().into_value();
    let bytes = crate::__rt::encode_value(&guest_value);
    let host_value: QueryValue = rmp_serde::from_slice(&bytes).unwrap();

    match &host_value {
        QueryValue::Map(m) => {
            let errors = m.get("errors").unwrap();
            match errors {
                QueryValue::List(items) => assert!(items.is_empty()),
                other => panic!("expected empty List, got: {other:?}"),
            }
            assert_eq!(*m.get("stop").unwrap(), QueryValue::Bool(false));
        }
        other => panic!("expected Map, got: {other:?}"),
    }
}

#[test]
fn vec_string_field_path() {
    let path: Vec<String> = vec!["items".into(), "0".into(), "sku".into()];
    let v = Validation::reject(path, "invalid_sku").into_value();
    let errors = extract_errors(&v);
    let field = map_get(&errors[0], "field").unwrap();
    assert_eq!(
        *field,
        Value::List(vec![
            Value::Str("items".into()),
            Value::Str("0".into()),
            Value::Str("sku".into()),
        ])
    );
}

#[test]
fn slice_field_path() {
    let segments: &[&str] = &["a", "b", "c"];
    let v = Validation::reject(segments, "x").into_value();
    let errors = extract_errors(&v);
    let field = map_get(&errors[0], "field").unwrap();
    assert_eq!(
        *field,
        Value::List(vec![
            Value::Str("a".into()),
            Value::Str("b".into()),
            Value::Str("c".into()),
        ])
    );
}
