//! Tests for `val::expr` / `val::add` / `val::sub` / … — every constructor is
//! verified against exact wire shape and round-tripped through msgpack.

use shamir_query_types::filter::{FilterExpr, FilterExprOp, FilterValue};
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

// ── generic expr(op, args) ───────────────────────────────────────────

#[test]
fn expr_generic_add() {
    // Structural: built via val::expr must equal hand-built DTO.
    let built = expr(FilterExprOp::Add, [col("a"), lit(1_i64)]);
    let hand = FilterValue::Expr {
        expr: FilterExpr::new(FilterExprOp::Add, vec![col("a"), lit(1_i64)]),
    };
    assert_eq!(built, hand);

    // Wire shape.
    assert_wire(
        built,
        mpack!({
            "$expr": {
                "op": "add",
                "args": [
                    { "$ref": ["a"] },
                    1
                ]
            }
        }),
    );
}

// ── arithmetic wrappers ──────────────────────────────────────────────

#[test]
fn add_arithmetic() {
    assert_wire(
        add(col("price"), lit(10_i64)),
        mpack!({
            "$expr": {
                "op": "add",
                "args": [{ "$ref": ["price"] }, 10]
            }
        }),
    );
}

#[test]
fn sub_arithmetic() {
    assert_wire(
        sub(col("total"), col("discount")),
        mpack!({
            "$expr": {
                "op": "sub",
                "args": [{ "$ref": ["total"] }, { "$ref": ["discount"] }]
            }
        }),
    );
}

#[test]
fn mul_arithmetic() {
    assert_wire(
        mul(col("qty"), lit(1.5_f64)),
        mpack!({
            "$expr": {
                "op": "mul",
                "args": [{ "$ref": ["qty"] }, 1.5]
            }
        }),
    );
}

#[test]
fn div_arithmetic() {
    assert_wire(
        div(col("sum"), lit(2_i64)),
        mpack!({
            "$expr": {
                "op": "div",
                "args": [{ "$ref": ["sum"] }, 2]
            }
        }),
    );
}

#[test]
fn modulo_arithmetic() {
    assert_wire(
        modulo(col("n"), lit(3_i64)),
        mpack!({
            "$expr": {
                "op": "mod",
                "args": [{ "$ref": ["n"] }, 3]
            }
        }),
    );
}

#[test]
fn neg_unary() {
    assert_wire(
        neg(col("balance")),
        mpack!({
            "$expr": {
                "op": "neg",
                "args": [{ "$ref": ["balance"] }]
            }
        }),
    );
}

// ── string wrappers ──────────────────────────────────────────────────

#[test]
fn concat_strings() {
    assert_wire(
        concat([col("first"), lit(" "), col("last")]),
        mpack!({
            "$expr": {
                "op": "concat",
                "args": [
                    { "$ref": ["first"] },
                    " ",
                    { "$ref": ["last"] }
                ]
            }
        }),
    );
}

#[test]
fn lower_string() {
    assert_wire(
        lower(col("email")),
        mpack!({
            "$expr": {
                "op": "lower",
                "args": [{ "$ref": ["email"] }]
            }
        }),
    );
}

#[test]
fn upper_string() {
    assert_wire(
        upper(col("name")),
        mpack!({
            "$expr": {
                "op": "upper",
                "args": [{ "$ref": ["name"] }]
            }
        }),
    );
}

#[test]
fn trim_string() {
    assert_wire(
        trim(col("raw")),
        mpack!({
            "$expr": {
                "op": "trim",
                "args": [{ "$ref": ["raw"] }]
            }
        }),
    );
}

#[test]
fn length_string() {
    assert_wire(
        length(col("name")),
        mpack!({
            "$expr": {
                "op": "length",
                "args": [{ "$ref": ["name"] }]
            }
        }),
    );
}

// ── logic wrappers ───────────────────────────────────────────────────

#[test]
fn and_expr_logic() {
    assert_wire(
        and_expr([col("active"), col("verified")]),
        mpack!({
            "$expr": {
                "op": "and",
                "args": [{ "$ref": ["active"] }, { "$ref": ["verified"] }]
            }
        }),
    );
}

#[test]
fn or_expr_logic() {
    assert_wire(
        or_expr([col("a"), col("b")]),
        mpack!({
            "$expr": {
                "op": "or",
                "args": [{ "$ref": ["a"] }, { "$ref": ["b"] }]
            }
        }),
    );
}

#[test]
fn not_expr_logic() {
    assert_wire(
        not_expr(col("deleted")),
        mpack!({
            "$expr": {
                "op": "not",
                "args": [{ "$ref": ["deleted"] }]
            }
        }),
    );
}

// ── nesting ──────────────────────────────────────────────────────────

#[test]
fn nested_expr() {
    // (price * qty) - discount
    let inner = mul(col("price"), col("qty"));
    let outer = sub(inner, col("discount"));
    assert_wire(
        outer,
        mpack!({
            "$expr": {
                "op": "sub",
                "args": [
                    {
                        "$expr": {
                            "op": "mul",
                            "args": [{ "$ref": ["price"] }, { "$ref": ["qty"] }]
                        }
                    },
                    { "$ref": ["discount"] }
                ]
            }
        }),
    );
}
