use crate::query::filter::eval::{compare_values, resolve_field};
use shamir_types::core::interner::Interner;
use shamir_types::types::value::InnerValue;

use super::helpers::{make_alice_record, make_nested_record};

#[test]
fn test_resolve_field_simple() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let k_name = interner.get_ind("name").unwrap().id();

    let val = resolve_field(&record, &[k_name]);
    assert_eq!(val, Some(InnerValue::Str("Alice".to_string())));
}

#[test]
fn test_resolve_field_nested() {
    let interner = Interner::new();
    let record = make_nested_record(&interner);
    let k_user = interner.get_ind("user").unwrap().id();
    let k_name = interner.get_ind("name").unwrap().id();

    let val = resolve_field(&record, &[k_user, k_name]);
    assert_eq!(val, Some(InnerValue::Str("Bob".to_string())));
}

#[test]
fn test_resolve_field_missing() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let k_missing = interner
        .touch_ind("nonexistent")
        .unwrap()
        .key()
        .clone()
        .id();

    let val = resolve_field(&record, &[k_missing]);
    assert_eq!(val, None);
}

#[test]
fn test_resolve_field_empty_path() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let val = resolve_field(&record, &[]);
    assert!(val.is_some());
}

#[test]
fn test_compare_values_int() {
    use std::cmp::Ordering;
    assert_eq!(
        compare_values(&InnerValue::Int(10), &InnerValue::Int(20)),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare_values(&InnerValue::Int(20), &InnerValue::Int(20)),
        Some(Ordering::Equal)
    );
    assert_eq!(
        compare_values(&InnerValue::Int(30), &InnerValue::Int(20)),
        Some(Ordering::Greater)
    );
}

#[test]
fn test_compare_values_str() {
    use std::cmp::Ordering;
    assert_eq!(
        compare_values(
            &InnerValue::Str("abc".into()),
            &InnerValue::Str("def".into())
        ),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare_values(
            &InnerValue::Str("abc".into()),
            &InnerValue::Str("abc".into())
        ),
        Some(Ordering::Equal)
    );
}

#[test]
fn test_compare_values_float() {
    use std::cmp::Ordering;
    assert_eq!(
        compare_values(&InnerValue::F64(1.0), &InnerValue::F64(2.0)),
        Some(Ordering::Less)
    );
}

#[test]
fn test_compare_values_int_float_cross() {
    use std::cmp::Ordering;
    assert_eq!(
        compare_values(&InnerValue::Int(10), &InnerValue::F64(10.5)),
        Some(Ordering::Less)
    );
}

#[test]
fn test_compare_values_null() {
    use std::cmp::Ordering;
    assert_eq!(
        compare_values(&InnerValue::Null, &InnerValue::Null),
        Some(Ordering::Equal)
    );
}

#[test]
fn test_compare_values_incompatible() {
    assert_eq!(
        compare_values(&InnerValue::Int(1), &InnerValue::Str("a".into())),
        None
    );
}
