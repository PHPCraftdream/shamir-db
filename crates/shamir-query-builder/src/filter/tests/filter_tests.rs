//! Tests for `filter` module — every leaf constructor, free combinator,
//! and `FilterExt` smart-merge is verified against exact wire shape and
//! round-tripped through msgpack.

use shamir_query_types::filter::Filter;
use shamir_types::mpack;

use crate::filter::*;
use crate::val::*;

// ── helpers ──────────────────────────────────────────────────────────

/// Serialize → msgpack-decoded QueryValue, assert equality against `mpack!`
/// expected, then round-trip the original type back and assert structural
/// equality.
fn assert_wire(f: Filter, expected: shamir_types::types::value::QueryValue) {
    let bytes = rmp_serde::to_vec_named(&f).expect("serialize");
    let got: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode QueryValue");
    assert_eq!(got, expected, "wire shape mismatch");
    let back: Filter = rmp_serde::from_slice(&bytes).expect("round-trip");
    assert_eq!(back, f, "round-trip mismatch");
}

// ── comparison leaves ────────────────────────────────────────────────

#[test]
fn test_eq() {
    assert_wire(
        eq("status", "active"),
        mpack!({
            "op": "eq",
            "field": ["status"],
            "value": "active"
        }),
    );
}

#[test]
fn test_ne() {
    assert_wire(
        ne("role", "guest"),
        mpack!({
            "op": "ne",
            "field": ["role"],
            "value": "guest"
        }),
    );
}

#[test]
fn test_gt() {
    assert_wire(
        gt("age", 18_i64),
        mpack!({
            "op": "gt",
            "field": ["age"],
            "value": 18
        }),
    );
}

#[test]
fn test_gte() {
    assert_wire(
        gte("score", 90_i64),
        mpack!({
            "op": "gte",
            "field": ["score"],
            "value": 90
        }),
    );
}

#[test]
fn test_lt() {
    assert_wire(
        lt("price", 100.0_f64),
        mpack!({
            "op": "lt",
            "field": ["price"],
            "value": 100.0
        }),
    );
}

#[test]
fn test_lte() {
    assert_wire(
        lte("qty", 5_i64),
        mpack!({
            "op": "lte",
            "field": ["qty"],
            "value": 5
        }),
    );
}

// ── field_eq ─────────────────────────────────────────────────────────

#[test]
fn test_field_eq() {
    assert_wire(
        field_eq("name", "alice"),
        mpack!({
            "op": "field",
            "field": ["name"],
            "value": "alice"
        }),
    );
}

// ── in / not_in ──────────────────────────────────────────────────────

#[test]
fn test_in() {
    assert_wire(
        in_("role", ["admin", "mod"]),
        mpack!({
            "op": "in",
            "field": ["role"],
            "values": ["admin", "mod"]
        }),
    );
}

#[test]
fn test_not_in() {
    assert_wire(
        not_in("status", ["deleted", "banned"]),
        mpack!({
            "op": "not_in",
            "field": ["status"],
            "values": ["deleted", "banned"]
        }),
    );
}

// ── pattern matching ─────────────────────────────────────────────────

#[test]
fn test_like() {
    assert_wire(
        like("name", "Al%"),
        mpack!({
            "op": "like",
            "field": ["name"],
            "pattern": "Al%"
        }),
    );
}

#[test]
fn test_ilike() {
    assert_wire(
        ilike("email", "%@example.com"),
        mpack!({
            "op": "i_like",
            "field": ["email"],
            "pattern": "%@example.com"
        }),
    );
}

#[test]
fn test_regex() {
    assert_wire(
        regex("code", "^[A-Z]{3}$"),
        mpack!({
            "op": "regex",
            "field": ["code"],
            "pattern": "^[A-Z]{3}$"
        }),
    );
}

// ── null / existence ─────────────────────────────────────────────────

#[test]
fn test_is_null() {
    assert_wire(
        is_null("deleted_at"),
        mpack!({
            "op": "is_null",
            "field": ["deleted_at"]
        }),
    );
}

#[test]
fn test_is_not_null() {
    assert_wire(
        is_not_null("email"),
        mpack!({
            "op": "is_not_null",
            "field": ["email"]
        }),
    );
}

#[test]
fn test_exists() {
    assert_wire(
        exists("profile"),
        mpack!({
            "op": "exists",
            "field": ["profile"]
        }),
    );
}

#[test]
fn test_not_exists() {
    assert_wire(
        not_exists("legacy_field"),
        mpack!({
            "op": "not_exists",
            "field": ["legacy_field"]
        }),
    );
}

// ── containment ──────────────────────────────────────────────────────

#[test]
fn test_contains() {
    assert_wire(
        contains("tags", "rust"),
        mpack!({
            "op": "contains",
            "field": ["tags"],
            "value": "rust"
        }),
    );
}

#[test]
fn test_contains_any() {
    assert_wire(
        contains_any("tags", ["rust", "go"]),
        mpack!({
            "op": "contains_any",
            "field": ["tags"],
            "values": ["rust", "go"]
        }),
    );
}

#[test]
fn test_contains_all() {
    assert_wire(
        contains_all("tags", ["db", "fast"]),
        mpack!({
            "op": "contains_all",
            "field": ["tags"],
            "values": ["db", "fast"]
        }),
    );
}

// ── between ──────────────────────────────────────────────────────────

#[test]
fn test_between() {
    assert_wire(
        between("age", 18_i64, 65_i64),
        mpack!({
            "op": "between",
            "field": ["age"],
            "from": 18,
            "to": 65
        }),
    );
}

// ── fts ──────────────────────────────────────────────────────────────

#[test]
fn test_fts() {
    assert_wire(
        fts("body", "hello world", "and"),
        mpack!({
            "op": "fts",
            "field": ["body"],
            "query": "hello world",
            "mode": "and"
        }),
    );
}

// ── vector_similarity ────────────────────────────────────────────────

#[test]
fn test_vector_similarity() {
    assert_wire(
        vector_similarity("embedding", vec![1.0, 0.0, 0.5], 10),
        mpack!({
            "op": "vector_similarity",
            "field": ["embedding"],
            "query": [1.0, 0.0, 0.5],
            "k": 10
        }),
    );
}

// ── computed (functional index) ──────────────────────────────────────

#[test]
fn test_computed_lower_eq() {
    assert_wire(
        computed("lower", "email", "eq", "alice@foo.com"),
        mpack!({
            "op": "computed",
            "expr_op": "lower",
            "field": ["email"],
            "cmp": "eq",
            "value": "alice@foo.com"
        }),
    );
}

#[test]
fn test_computed_with_args() {
    assert_wire(
        computed_with_args("substring", "name", [lit(0_i64), lit(3_i64)], "eq", "ali"),
        mpack!({
            "op": "computed",
            "expr_op": "substring",
            "field": ["name"],
            "expr_args": [0, 3],
            "cmp": "eq",
            "value": "ali"
        }),
    );
}

#[test]
fn test_computed_nested_field() {
    assert_wire(
        computed("lower", ["address", "city"], "eq", "ny"),
        mpack!({
            "op": "computed",
            "expr_op": "lower",
            "field": ["address", "city"],
            "cmp": "eq",
            "value": "ny"
        }),
    );
}

// ── free combinators ─────────────────────────────────────────────────

#[test]
fn test_and() {
    assert_wire(
        and([eq("a", "x"), gt("b", 1_i64)]),
        mpack!({
            "op": "and",
            "filters": [
                {
                    "op": "eq",
                    "field": ["a"],
                    "value": "x"
                },
                {
                    "op": "gt",
                    "field": ["b"],
                    "value": 1
                }
            ]
        }),
    );
}

#[test]
fn test_or() {
    assert_wire(
        or([eq("x", true), eq("y", false)]),
        mpack!({
            "op": "or",
            "filters": [
                {
                    "op": "eq",
                    "field": ["x"],
                    "value": true
                },
                {
                    "op": "eq",
                    "field": ["y"],
                    "value": false
                }
            ]
        }),
    );
}

#[test]
fn test_not() {
    assert_wire(
        not(eq("status", "deleted")),
        mpack!({
            "op": "not",
            "filter": {
                "op": "eq",
                "field": ["status"],
                "value": "deleted"
            }
        }),
    );
}

// ── FilterExt smart merge ────────────────────────────────────────────

#[test]
fn ext_and_creates_pair() {
    let f = eq("a", 1_i64).and(eq("b", 2_i64));
    assert_wire(
        f,
        mpack!({
            "op": "and",
            "filters": [
                {
                    "op": "eq",
                    "field": ["a"],
                    "value": 1
                },
                {
                    "op": "eq",
                    "field": ["b"],
                    "value": 2
                }
            ]
        }),
    );
}

#[test]
fn ext_and_flattens_existing_and() {
    let f = eq("a", 1_i64).and(eq("b", 2_i64)).and(eq("c", 3_i64));
    // Should be flat: And{[a,b,c]}, not And{[And{[a,b]}, c]}
    match &f {
        Filter::And { filters } => assert_eq!(filters.len(), 3),
        other => panic!("expected And, got {other:?}"),
    }
}

#[test]
fn ext_or_creates_pair() {
    let f = eq("a", 1_i64).or(eq("b", 2_i64));
    assert_wire(
        f,
        mpack!({
            "op": "or",
            "filters": [
                {
                    "op": "eq",
                    "field": ["a"],
                    "value": 1
                },
                {
                    "op": "eq",
                    "field": ["b"],
                    "value": 2
                }
            ]
        }),
    );
}

#[test]
fn ext_or_flattens_existing_or() {
    let f = eq("a", 1_i64).or(eq("b", 2_i64)).or(eq("c", 3_i64));
    match &f {
        Filter::Or { filters } => assert_eq!(filters.len(), 3),
        other => panic!("expected Or, got {other:?}"),
    }
}

#[test]
fn ext_negate() {
    let f = eq("active", true).negate();
    assert_wire(
        f,
        mpack!({
            "op": "not",
            "filter": {
                "op": "eq",
                "field": ["active"],
                "value": true
            }
        }),
    );
}

// ── nested path in leaves ────────────────────────────────────────────

#[test]
fn eq_nested_field() {
    assert_wire(
        eq(["address", "city"], "NY"),
        mpack!({
            "op": "eq",
            "field": ["address", "city"],
            "value": "NY"
        }),
    );
}

// ── FilterValue::FnCall in a filter value slot ───────────────────────

#[test]
fn eq_with_func_value() {
    assert_wire(
        eq("name", func("strings/lower", [lit("ALICE")])),
        mpack!({
            "op": "eq",
            "field": ["name"],
            "value": {
                "$fn": {
                    "name": "strings/lower",
                    "args": ["ALICE"]
                }
            }
        }),
    );
}

// ── complex composition ──────────────────────────────────────────────

#[test]
fn complex_and_or_not_tree() {
    // (status = 'active') AND (role = 'admin' OR vip = true)
    let f = eq("status", "active").and(or([eq("role", "admin"), eq("vip", true)]));
    assert_wire(
        f,
        mpack!({
            "op": "and",
            "filters": [
                {
                    "op": "eq",
                    "field": ["status"],
                    "value": "active"
                },
                {
                    "op": "or",
                    "filters": [
                        {
                            "op": "eq",
                            "field": ["role"],
                            "value": "admin"
                        },
                        {
                            "op": "eq",
                            "field": ["vip"],
                            "value": true
                        }
                    ]
                }
            ]
        }),
    );
}
