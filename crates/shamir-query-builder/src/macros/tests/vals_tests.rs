//! Tests for the `vals!` macro.

use shamir_query_types::filter::FilterValue;

use crate::val::lit;

#[test]
fn vals_macro_ints() {
    let from_macro: Vec<FilterValue> = vals![1, 2, 3];
    let from_builder: Vec<FilterValue> = vec![lit(1), lit(2), lit(3)];
    assert_eq!(from_macro, from_builder);
}

#[test]
fn vals_macro_mixed() {
    let from_macro: Vec<FilterValue> = vals!["a", 42, true];
    let from_builder: Vec<FilterValue> = vec![lit("a"), lit(42), lit(true)];
    assert_eq!(from_macro, from_builder);
}

#[test]
fn vals_macro_empty() {
    let from_macro: Vec<FilterValue> = vals![];
    let from_builder: Vec<FilterValue> = Vec::new();
    assert_eq!(from_macro, from_builder);
}

#[test]
fn vals_macro_trailing_comma() {
    let from_macro: Vec<FilterValue> = vals![1, 2,];
    let from_builder: Vec<FilterValue> = vec![lit(1), lit(2)];
    assert_eq!(from_macro, from_builder);
}

#[test]
fn vals_macro_wire_json() {
    let v = vals![1, "hello", true];
    let json = serde_json::to_value(&v).unwrap();
    let expected = serde_json::json!([1, "hello", true]);
    assert_eq!(json, expected);
}
