//! Tests for the `doc!` macro.

use serde_json::Value;

use crate::val::col;
use crate::write::doc;

#[test]
fn doc_macro_simple() {
    let from_macro: Value = doc! {
        "a" => 1,
        "b" => col("x"),
    }
    .build();

    let from_builder: Value = doc().set("a", 1).set("b", col("x")).build();

    assert_eq!(from_macro, from_builder);
}

#[test]
fn doc_macro_empty() {
    let from_macro: Value = doc! {}.build();
    let from_builder: Value = doc().build();
    assert_eq!(from_macro, from_builder);
}

#[test]
fn doc_macro_trailing_comma() {
    let from_macro: Value = doc! {
        "k" => "v",
    }
    .build();

    let from_builder: Value = doc().set("k", "v").build();
    assert_eq!(from_macro, from_builder);
}

#[test]
fn doc_macro_wire_json() {
    let d = doc! {
        "a" => 1,
        "b" => col("x"),
    };
    let json = serde_json::to_value(d.build()).unwrap();

    let expected = serde_json::json!({
        "a": 1,
        "b": {"$ref": ["x"]}
    });
    assert_eq!(json, expected);
}
