//! Tests for `val::cond` — verified against exact wire shape and round-tripped
//! through msgpack.

use shamir_query_types::filter::{Cond, Filter, FilterValue};
use shamir_types::mpack;

use crate::val::*;

/// Serialize → msgpack-decoded QueryValue, assert equality, then round-trip
/// the original FilterValue back and assert structural equality. Mirrors the
/// helper in `val_tests`.
fn assert_wire(fv: FilterValue, expected: shamir_types::types::value::QueryValue) {
    let bytes = rmp_serde::to_vec_named(&fv).expect("serialize");
    let got: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode QueryValue");
    assert_eq!(got, expected, "wire shape mismatch");
    let back: FilterValue = rmp_serde::from_slice(&bytes).expect("round-trip");
    assert_eq!(back, fv, "round-trip mismatch");
}

#[test]
fn cond_basic() {
    // Structural: built via val::cond must equal hand-built DTO.
    let condition = Filter::Eq {
        field: vec!["active".to_owned()],
        value: lit(true),
    };
    let built = cond(condition.clone(), lit("yes"), lit("no"));
    let hand = FilterValue::Cond {
        cond: Box::new(Cond::new(condition, lit("yes"), lit("no"))),
    };
    assert_eq!(built, hand);

    // Wire shape.
    assert_wire(
        built,
        mpack!({
            "$cond": {
                "if": {
                    "op": "eq",
                    "field": ["active"],
                    "value": true
                },
                "then": "yes",
                "else": "no"
            }
        }),
    );
}

#[test]
fn cond_with_comparison() {
    let condition = Filter::Gte {
        field: vec!["score".to_owned()],
        value: lit(100_i64),
    };
    assert_wire(
        cond(condition, lit("vip"), lit("regular")),
        mpack!({
            "$cond": {
                "if": {
                    "op": "gte",
                    "field": ["score"],
                    "value": 100
                },
                "then": "vip",
                "else": "regular"
            }
        }),
    );
}

#[test]
fn cond_nested_else() {
    // Outer cond's else branch is itself a cond.
    let inner_cond = Filter::Gte {
        field: vec!["score".to_owned()],
        value: lit(50_i64),
    };
    let inner = cond(inner_cond, lit("regular"), lit("newbie"));

    let outer_cond = Filter::Gte {
        field: vec!["score".to_owned()],
        value: lit(100_i64),
    };
    assert_wire(
        cond(outer_cond, lit("vip"), inner),
        mpack!({
            "$cond": {
                "if": {
                    "op": "gte",
                    "field": ["score"],
                    "value": 100
                },
                "then": "vip",
                "else": {
                    "$cond": {
                        "if": {
                            "op": "gte",
                            "field": ["score"],
                            "value": 50
                        },
                        "then": "regular",
                        "else": "newbie"
                    }
                }
            }
        }),
    );
}
