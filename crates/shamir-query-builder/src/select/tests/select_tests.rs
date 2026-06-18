//! Tests for `select` module — every constructor is verified against exact
//! wire shape and round-tripped through msgpack.

use shamir_query_types::read::SelectItem;
use shamir_types::mpack;

use crate::select::*;
use crate::val::{col, func as vfunc};

// ── helpers ──────────────────────────────────────────────────────────

/// Serialize → msgpack-decoded QueryValue, assert equality, then round-trip
/// the original type back and assert structural equality.
fn assert_wire(item: SelectItem, expected: shamir_types::types::value::QueryValue) {
    let bytes = rmp_serde::to_vec_named(&item).expect("serialize");
    let got: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode QueryValue");
    assert_eq!(got, expected, "wire shape mismatch");
    let back: SelectItem = rmp_serde::from_slice(&bytes).expect("round-trip");
    assert_eq!(back, item, "round-trip mismatch");
}

// ── all ──────────────────────────────────────────────────────────────

#[test]
fn test_all() {
    assert_wire(
        all(),
        mpack!({
            "type": "all"
        }),
    );
}

// ── field ────────────────────────────────────────────────────────────

#[test]
fn test_field_single_segment() {
    assert_wire(
        field("name"),
        mpack!({
            "type": "field",
            "path": ["name"]
        }),
    );
}

#[test]
fn test_field_nested_path() {
    assert_wire(
        field(["address", "zip"]),
        mpack!({
            "type": "field",
            "path": ["address", "zip"]
        }),
    );
}

#[test]
fn test_field_as() {
    assert_wire(
        field_as("email", "user_email"),
        mpack!({
            "type": "field",
            "path": ["email"],
            "alias": "user_email"
        }),
    );
}

// ── func (scalar function in projection) ────────────────────────────

#[test]
fn test_func() {
    assert_wire(
        func("up", "strings/upper", [col("name")]),
        mpack!({
            "type": "function",
            "name": "strings/upper",
            "args": [
                {
                    "$ref": ["name"]
                }
            ],
            "alias": "up"
        }),
    );
}

#[test]
fn test_func_nested_args() {
    assert_wire(
        func(
            "greeting",
            "strings/concat",
            [vfunc("strings/upper", [col("name")]), crate::val::lit("!")],
        ),
        mpack!({
            "type": "function",
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
            ],
            "alias": "greeting"
        }),
    );
}

// ── count_all ────────────────────────────────────────────────────────

#[test]
fn test_count_all() {
    assert_wire(
        count_all("n"),
        mpack!({
            "type": "count_all",
            "alias": "n"
        }),
    );
}

// ── agg (built-in aggregates) ───────────────────────────────────────

#[test]
fn test_agg_sum() {
    assert_wire(
        agg(AggFunc::Sum, "amount", "total"),
        mpack!({
            "type": "aggregate",
            "func": "sum",
            "field": ["amount"],
            "alias": "total",
            "distinct": false
        }),
    );
}

#[test]
fn test_agg_distinct() {
    assert_wire(
        agg_distinct(AggFunc::Count, "email", "unique_emails"),
        mpack!({
            "type": "aggregate",
            "func": "count",
            "field": ["email"],
            "alias": "unique_emails",
            "distinct": true
        }),
    );
}

// ── convenience wrappers ────────────────────────────────────────────

#[test]
fn test_sum() {
    assert_wire(
        sum("amount", "total"),
        mpack!({
            "type": "aggregate",
            "func": "sum",
            "field": ["amount"],
            "alias": "total",
            "distinct": false
        }),
    );
}

#[test]
fn test_avg() {
    assert_wire(
        avg("score", "mean_score"),
        mpack!({
            "type": "aggregate",
            "func": "avg",
            "field": ["score"],
            "alias": "mean_score",
            "distinct": false
        }),
    );
}

#[test]
fn test_min() {
    assert_wire(
        min("price", "cheapest"),
        mpack!({
            "type": "aggregate",
            "func": "min",
            "field": ["price"],
            "alias": "cheapest",
            "distinct": false
        }),
    );
}

#[test]
fn test_max() {
    assert_wire(
        max("price", "most_expensive"),
        mpack!({
            "type": "aggregate",
            "func": "max",
            "field": ["price"],
            "alias": "most_expensive",
            "distinct": false
        }),
    );
}

#[test]
fn test_count() {
    assert_wire(
        count("user_id", "user_count"),
        mpack!({
            "type": "aggregate",
            "func": "count",
            "field": ["user_id"],
            "alias": "user_count",
            "distinct": false
        }),
    );
}

// ── agg_fn (funclib aggregates) ─────────────────────────────────────

#[test]
fn test_agg_fn() {
    assert_wire(
        agg_fn("median", "age", "med"),
        mpack!({
            "type": "aggregate_fn",
            "name": "median",
            "field": ["age"],
            "alias": "med",
            "distinct": false
        }),
    );
}

#[test]
fn test_agg_fn_distinct() {
    assert_wire(
        agg_fn_distinct("count_distinct", "category", "uniq_cats"),
        mpack!({
            "type": "aggregate_fn",
            "name": "count_distinct",
            "field": ["category"],
            "alias": "uniq_cats",
            "distinct": true
        }),
    );
}

// ── nested field paths in aggregates ────────────────────────────────

#[test]
fn test_agg_nested_path() {
    assert_wire(
        sum(["order", "amount"], "order_total"),
        mpack!({
            "type": "aggregate",
            "func": "sum",
            "field": ["order", "amount"],
            "alias": "order_total",
            "distinct": false
        }),
    );
}

#[test]
fn test_agg_fn_nested_path() {
    assert_wire(
        agg_fn("stddev", ["stats", "latency"], "lat_sd"),
        mpack!({
            "type": "aggregate_fn",
            "name": "stddev",
            "field": ["stats", "latency"],
            "alias": "lat_sd",
            "distinct": false
        }),
    );
}
