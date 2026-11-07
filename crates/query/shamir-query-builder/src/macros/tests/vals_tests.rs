//! Tests for the `vals!` macro.

use shamir_query_types::filter::FilterValue;
use shamir_types::mpack;

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
fn vals_macro_wire_shape() {
    let v = vals![1, "hello", true];
    let bytes = rmp_serde::to_vec_named(&v).expect("serialize");
    let got: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode");
    let expected = mpack!([1, "hello", true]);
    assert_eq!(got, expected);
}
