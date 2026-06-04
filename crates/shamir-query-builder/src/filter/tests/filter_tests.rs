//! Tests for `filter` module — every leaf constructor, free combinator,
//! and `FilterExt` smart-merge is verified against exact wire JSON and
//! round-tripped.

use serde_json::{json, Value};
use shamir_query_types::filter::Filter;

use crate::filter::*;
use crate::val::*;

// ── helpers ──────────────────────────────────────────────────────────

/// Serialize → JSON value, assert equality, then round-trip back.
fn assert_wire(f: Filter, expected: Value) {
    let got = serde_json::to_value(&f).unwrap();
    assert_eq!(got, expected, "wire JSON mismatch");
    let back: Filter = serde_json::from_value(got).unwrap();
    assert_eq!(back, f, "round-trip mismatch");
}

// ── comparison leaves ────────────────────────────────────────────────

#[test]
fn test_eq() {
    assert_wire(
        eq("status", "active"),
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
            "op": "is_null",
            "field": ["deleted_at"]
        }),
    );
}

#[test]
fn test_is_not_null() {
    assert_wire(
        is_not_null("email"),
        json!({
            "op": "is_not_null",
            "field": ["email"]
        }),
    );
}

#[test]
fn test_exists() {
    assert_wire(
        exists("profile"),
        json!({
            "op": "exists",
            "field": ["profile"]
        }),
    );
}

#[test]
fn test_not_exists() {
    assert_wire(
        not_exists("legacy_field"),
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
            "op": "vector_similarity",
            "field": ["embedding"],
            "query": [1.0, 0.0, 0.5],
            "k": 10
        }),
    );
}

// ── free combinators ─────────────────────────────────────────────────

#[test]
fn test_and() {
    assert_wire(
        and([eq("a", "x"), gt("b", 1_i64)]),
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
        json!({
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
