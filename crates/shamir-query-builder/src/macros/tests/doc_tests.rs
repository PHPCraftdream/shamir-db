//! Tests for the `doc!` macro.

use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::val::col;
use crate::write::doc;

#[test]
fn doc_macro_simple() {
    let from_macro: QueryValue = doc! {
        "a" => 1,
        "b" => col("x"),
    }
    .build();

    let from_builder: QueryValue = doc().set("a", 1).set("b", col("x")).build();

    assert_eq!(from_macro, from_builder);
}

#[test]
fn doc_macro_empty() {
    let from_macro: QueryValue = doc! {}.build();
    let from_builder: QueryValue = doc().build();
    assert_eq!(from_macro, from_builder);
}

#[test]
fn doc_macro_trailing_comma() {
    let from_macro: QueryValue = doc! {
        "k" => "v",
    }
    .build();

    let from_builder: QueryValue = doc().set("k", "v").build();
    assert_eq!(from_macro, from_builder);
}

#[test]
fn doc_macro_wire_msgpack() {
    let d = doc! {
        "a" => 1,
        "b" => col("x"),
    };
    let bytes = rmp_serde::to_vec_named(&d.build()).unwrap();
    let got: QueryValue = rmp_serde::from_slice(&bytes).unwrap();

    let expected = mpack!({
        "a": 1,
        "b": {"$ref": ["x"]}
    });
    assert_eq!(got, expected);
}
