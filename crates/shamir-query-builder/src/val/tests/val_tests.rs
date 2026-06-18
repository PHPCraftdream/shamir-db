//! Tests for `val` module — every constructor is verified against exact
//! wire shape and round-tripped through msgpack.

use shamir_query_types::filter::FilterValue;
use shamir_types::mpack;

use crate::val::*;

// ── helpers ──────────────────────────────────────────────────────────

/// Serialize → msgpack-decoded QueryValue, assert equality, then round-trip
/// the original type back and assert structural equality.
fn assert_wire(fv: FilterValue, expected: shamir_types::types::value::QueryValue) {
    let bytes = rmp_serde::to_vec_named(&fv).expect("serialize");
    let got: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode QueryValue");
    assert_eq!(got, expected, "wire shape mismatch");
    let back: FilterValue = rmp_serde::from_slice(&bytes).expect("round-trip");
    assert_eq!(back, fv, "round-trip mismatch");
}

// ── lit / From impls ─────────────────────────────────────────────────

#[test]
fn lit_i64() {
    assert_wire(lit(42_i64), mpack!(42));
}

#[test]
fn lit_i32() {
    assert_wire(lit(42_i32), mpack!(42));
}

#[test]
fn lit_u32() {
    assert_wire(lit(100_u32), mpack!(100));
}

#[test]
fn lit_i16() {
    assert_wire(lit(7_i16), mpack!(7));
}

#[test]
fn lit_i8() {
    assert_wire(lit(3_i8), mpack!(3));
}

#[test]
fn lit_u8() {
    assert_wire(lit(255_u8), mpack!(255));
}

#[test]
fn lit_u16() {
    assert_wire(lit(1000_u16), mpack!(1000));
}

#[test]
fn test_lit_u64() {
    assert_wire(lit_u64(999), mpack!(999));
}

#[test]
fn lit_f64() {
    assert_wire(lit(2.72_f64), mpack!(2.72));
}

#[test]
fn lit_f32() {
    assert_wire(lit(1.5_f32), mpack!(1.5));
}

#[test]
fn lit_bool() {
    assert_wire(lit(true), mpack!(true));
    assert_wire(lit(false), mpack!(false));
}

#[test]
fn lit_string() {
    assert_wire(lit("hello"), mpack!("hello"));
}

#[test]
fn lit_owned_string() {
    assert_wire(lit(String::from("world")), mpack!("world"));
}

// ── From impl msgpack round-trip ────────────────────────────────────

#[test]
fn from_i32_msgpack() {
    let fv = FilterValue::from(18_i32);
    let bytes = rmp_serde::to_vec_named(&fv).expect("serialize");
    let got: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(got, mpack!(18));
}

#[test]
fn from_f32_msgpack() {
    let fv = FilterValue::from(2.5_f32);
    let bytes = rmp_serde::to_vec_named(&fv).expect("serialize");
    let got: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(got, mpack!(2.5));
}

#[test]
fn from_u16_msgpack() {
    let fv = FilterValue::from(500_u16);
    let bytes = rmp_serde::to_vec_named(&fv).expect("serialize");
    let got: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(got, mpack!(500));
}

// ── null / binary ────────────────────────────────────────────────────

#[test]
fn null_value() {
    assert_wire(null(), mpack!(null));
}

#[test]
fn binary_value() {
    let fv = bin(vec![0xDE, 0xAD]);
    let bytes = rmp_serde::to_vec_named(&fv).expect("serialize");
    let got: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode QueryValue");
    // Binary serializes as Bin variant.
    assert_eq!(
        got,
        shamir_types::types::value::QueryValue::Bin(vec![0xDE, 0xAD])
    );
}

// ── col (FieldRef) ───────────────────────────────────────────────────

#[test]
fn col_single_segment() {
    assert_wire(
        col("email"),
        mpack!({
            "$ref": ["email"]
        }),
    );
}

#[test]
fn col_nested_path() {
    assert_wire(
        col(["address", "zip"]),
        mpack!({
            "$ref": ["address", "zip"]
        }),
    );
}

#[test]
fn col_vec_string() {
    assert_wire(
        col(vec!["a".to_owned(), "b".to_owned()]),
        mpack!({
            "$ref": ["a", "b"]
        }),
    );
}

#[test]
fn col_vec_str() {
    assert_wire(
        col(vec!["x", "y"]),
        mpack!({
            "$ref": ["x", "y"]
        }),
    );
}

#[test]
fn col_slice() {
    let segments: &[&str] = &["p", "q"];
    assert_wire(
        col(segments),
        mpack!({
            "$ref": ["p", "q"]
        }),
    );
}

// ── func (FnCall) ────────────────────────────────────────────────────

#[test]
fn func_no_args() {
    assert_wire(
        func("NOW", []),
        mpack!({
            "$fn": {
                "name": "NOW"
            }
        }),
    );
}

#[test]
fn func_with_args() {
    assert_wire(
        func("strings/lower", [col("email")]),
        mpack!({
            "$fn": {
                "name": "strings/lower",
                "args": [
                    {
                        "$ref": ["email"]
                    }
                ]
            }
        }),
    );
}

#[test]
fn func_nested() {
    let inner = func("strings/upper", [col("name")]);
    let outer = func("strings/concat", [inner, lit("!")]);
    assert_wire(
        outer,
        mpack!({
            "$fn": {
                "name": "strings/concat",
                "args": [
                    {
                        "$fn": {
                            "name": "strings/upper",
                            "args": [
                                {
                                    "$ref": ["name"]
                                }
                            ]
                        }
                    },
                    "!"
                ]
            }
        }),
    );
}

// ── qref / qref_all (QueryRef) ──────────────────────────────────────

#[test]
fn qref_with_at_prefix() {
    assert_wire(
        qref("@users", "[].id"),
        mpack!({
            "$query": "@users",
            "path": "[].id"
        }),
    );
}

#[test]
fn qref_auto_prepends_at() {
    assert_wire(
        qref("users", "[].id"),
        mpack!({
            "$query": "@users",
            "path": "[].id"
        }),
    );
}

#[test]
fn qref_all_with_at() {
    assert_wire(
        qref_all("@orders"),
        mpack!({
            "$query": "@orders"
        }),
    );
}

#[test]
fn qref_all_auto_at() {
    assert_wire(
        qref_all("orders"),
        mpack!({
            "$query": "@orders"
        }),
    );
}

// ── array ────────────────────────────────────────────────────────────

#[test]
fn array_via_vec() {
    let arr: Vec<FilterValue> = vec![lit(1_i64), lit("two"), lit(true)];
    let fv = FilterValue::from(arr);
    assert_wire(fv, mpack!([1, "two", true]));
}

// ── IntoFieldPath ────────────────────────────────────────────────────

#[test]
fn into_field_path_str() {
    let p: Vec<String> = "name".into_field_path();
    assert_eq!(p, vec!["name".to_owned()]);
}

#[test]
fn into_field_path_string() {
    let p: Vec<String> = String::from("age").into_field_path();
    assert_eq!(p, vec!["age".to_owned()]);
}

#[test]
fn into_field_path_array() {
    let p: Vec<String> = ["a", "b", "c"].into_field_path();
    assert_eq!(p, vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]);
}
