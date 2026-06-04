//! Tests for `val` module — every constructor is verified against exact
//! wire JSON and round-tripped through serde.

use serde_json::{json, Value};
use shamir_query_types::filter::FilterValue;

use crate::val::*;

// ── helpers ──────────────────────────────────────────────────────────

/// Serialize → JSON value, assert equality, then round-trip back.
fn assert_wire(fv: FilterValue, expected: Value) {
    let got = serde_json::to_value(&fv).unwrap();
    assert_eq!(got, expected, "wire JSON mismatch");
    let back: FilterValue = serde_json::from_value(got).unwrap();
    assert_eq!(back, fv, "round-trip mismatch");
}

// ── lit / From impls ─────────────────────────────────────────────────

#[test]
fn lit_i64() {
    assert_wire(lit(42_i64), json!(42));
}

#[test]
fn lit_i32() {
    assert_wire(lit(42_i32), json!(42));
}

#[test]
fn lit_u32() {
    assert_wire(lit(100_u32), json!(100));
}

#[test]
fn lit_i16() {
    assert_wire(lit(7_i16), json!(7));
}

#[test]
fn lit_i8() {
    assert_wire(lit(3_i8), json!(3));
}

#[test]
fn lit_u8() {
    assert_wire(lit(255_u8), json!(255));
}

#[test]
fn lit_u16() {
    assert_wire(lit(1000_u16), json!(1000));
}

#[test]
fn test_lit_u64() {
    assert_wire(lit_u64(999), json!(999));
}

#[test]
fn lit_f64() {
    assert_wire(lit(2.72_f64), json!(2.72));
}

#[test]
fn lit_f32() {
    assert_wire(lit(1.5_f32), json!(1.5));
}

#[test]
fn lit_bool() {
    assert_wire(lit(true), json!(true));
    assert_wire(lit(false), json!(false));
}

#[test]
fn lit_string() {
    assert_wire(lit("hello"), json!("hello"));
}

#[test]
fn lit_owned_string() {
    assert_wire(lit(String::from("world")), json!("world"));
}

// ── From impl serde round-trip ──────────────────────────────────────

#[test]
fn from_i32_serde_json() {
    assert_eq!(
        serde_json::to_value(FilterValue::from(18_i32)).unwrap(),
        json!(18)
    );
}

#[test]
fn from_f32_serde_json() {
    assert_eq!(
        serde_json::to_value(FilterValue::from(2.5_f32)).unwrap(),
        json!(2.5)
    );
}

#[test]
fn from_u16_serde_json() {
    assert_eq!(
        serde_json::to_value(FilterValue::from(500_u16)).unwrap(),
        json!(500)
    );
}

// ── null / binary ────────────────────────────────────────────────────

#[test]
fn null_value() {
    assert_wire(null(), json!(null));
}

#[test]
fn binary_value() {
    let fv = bin(vec![0xDE, 0xAD]);
    let got = serde_json::to_value(&fv).unwrap();
    // Binary serializes as an array of bytes.
    assert_eq!(got, json!([222, 173]));
}

// ── col (FieldRef) ───────────────────────────────────────────────────

#[test]
fn col_single_segment() {
    assert_wire(
        col("email"),
        json!({
            "$ref": ["email"]
        }),
    );
}

#[test]
fn col_nested_path() {
    assert_wire(
        col(["address", "zip"]),
        json!({
            "$ref": ["address", "zip"]
        }),
    );
}

#[test]
fn col_vec_string() {
    assert_wire(
        col(vec!["a".to_owned(), "b".to_owned()]),
        json!({
            "$ref": ["a", "b"]
        }),
    );
}

#[test]
fn col_vec_str() {
    assert_wire(
        col(vec!["x", "y"]),
        json!({
            "$ref": ["x", "y"]
        }),
    );
}

#[test]
fn col_slice() {
    let segments: &[&str] = &["p", "q"];
    assert_wire(
        col(segments),
        json!({
            "$ref": ["p", "q"]
        }),
    );
}

// ── func (FnCall) ────────────────────────────────────────────────────

#[test]
fn func_no_args() {
    assert_wire(
        func("NOW", []),
        json!({
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
        json!({
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
        json!({
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
        json!({
            "$query": "@users",
            "path": "[].id"
        }),
    );
}

#[test]
fn qref_auto_prepends_at() {
    assert_wire(
        qref("users", "[].id"),
        json!({
            "$query": "@users",
            "path": "[].id"
        }),
    );
}

#[test]
fn qref_all_with_at() {
    assert_wire(
        qref_all("@orders"),
        json!({
            "$query": "@orders"
        }),
    );
}

#[test]
fn qref_all_auto_at() {
    assert_wire(
        qref_all("orders"),
        json!({
            "$query": "@orders"
        }),
    );
}

// ── array ────────────────────────────────────────────────────────────

#[test]
fn array_via_vec() {
    let arr: Vec<FilterValue> = vec![lit(1_i64), lit("two"), lit(true)];
    let fv = FilterValue::from(arr);
    assert_wire(fv, json!([1, "two", true]));
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
