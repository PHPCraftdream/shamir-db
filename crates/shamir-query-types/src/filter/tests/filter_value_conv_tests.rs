//! Unit tests for `query_value_to_filter_value` and `filter_value_to_query_value`.
//!
//! Verifies:
//! - Literal variants convert directly (no msgpack).
//! - `List` converts recursively.
//! - Exotic variants (`Map`, `Set`, `Dec`, `Big`) → `None`.
//! - Symmetric round-trip: `query_value_to_filter_value(filter_value_to_query_value(lit)) == lit`.
//! - `From<QueryValue> for FilterValue` uses the direct path for literals
//!   (no silent Null) and falls back to msgpack for Map/expression defaults.

use shamir_types::types::value::QueryValue;

use crate::filter::filter_value::{filter_value_to_query_value, query_value_to_filter_value};
use crate::filter::FilterValue;

// ── direct literal conversions ───────────────────────────────────────────────

#[test]
fn qv_to_fv_null() {
    let result = query_value_to_filter_value(&QueryValue::Null);
    assert_eq!(result, Some(FilterValue::Null));
}

#[test]
fn qv_to_fv_bool_true() {
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Bool(true)),
        Some(FilterValue::Bool(true))
    );
}

#[test]
fn qv_to_fv_bool_false() {
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Bool(false)),
        Some(FilterValue::Bool(false))
    );
}

#[test]
fn qv_to_fv_int() {
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Int(42)),
        Some(FilterValue::Int(42))
    );
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Int(-999)),
        Some(FilterValue::Int(-999))
    );
}

#[test]
fn qv_to_fv_f64() {
    // Use 1.5 (exact in IEEE 754) to avoid clippy::approx_constant.
    let result = query_value_to_filter_value(&QueryValue::F64(1.5));
    match result {
        Some(FilterValue::Float(f)) => assert!((f - 1.5).abs() < 1e-10),
        other => panic!("expected Float, got {:?}", other),
    }
}

#[test]
fn qv_to_fv_str() {
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Str("hello".to_string())),
        Some(FilterValue::String("hello".to_string()))
    );
}

#[test]
fn qv_to_fv_bin() {
    let bytes = vec![1u8, 2, 3];
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Bin(bytes.clone())),
        Some(FilterValue::Binary(bytes))
    );
}

// ── recursive List conversion ────────────────────────────────────────────────

#[test]
fn qv_to_fv_list_recursive() {
    let qv = QueryValue::List(vec![
        QueryValue::Int(1),
        QueryValue::Str("x".to_string()),
        QueryValue::Bool(false),
    ]);
    let expected = FilterValue::Array(vec![
        FilterValue::Int(1),
        FilterValue::String("x".to_string()),
        FilterValue::Bool(false),
    ]);
    assert_eq!(query_value_to_filter_value(&qv), Some(expected));
}

#[test]
fn qv_to_fv_nested_list() {
    let inner = QueryValue::List(vec![QueryValue::Int(10), QueryValue::Int(20)]);
    let outer = QueryValue::List(vec![inner, QueryValue::Null]);
    let result = query_value_to_filter_value(&outer);
    let expected = FilterValue::Array(vec![
        FilterValue::Array(vec![FilterValue::Int(10), FilterValue::Int(20)]),
        FilterValue::Null,
    ]);
    assert_eq!(result, Some(expected));
}

// ── exotic variants → None ───────────────────────────────────────────────────

#[test]
fn qv_to_fv_map_returns_none() {
    use shamir_types::types::common::new_map;
    let mut m = new_map();
    m.insert("$fn".to_string(), QueryValue::Str("now".to_string()));
    let qv = QueryValue::Map(m);
    // Map has no direct FilterValue equivalent → None (use msgpack fallback).
    assert!(query_value_to_filter_value(&qv).is_none());
}

#[test]
fn qv_to_fv_set_returns_none() {
    use shamir_types::types::common::TSet;
    // Set has no direct FilterValue equivalent → None.
    let qv = QueryValue::Set(TSet::default());
    assert!(query_value_to_filter_value(&qv).is_none());
}

// ── symmetric round-trip ─────────────────────────────────────────────────────

/// For every literal FilterValue, the round-trip
/// `qv_to_fv(fv_to_qv(fv)) == Some(fv)` must hold.
#[test]
fn round_trip_literals_symmetric() {
    let literals: Vec<FilterValue> = vec![
        FilterValue::Null,
        FilterValue::Bool(true),
        FilterValue::Bool(false),
        FilterValue::Int(0),
        FilterValue::Int(i64::MIN),
        FilterValue::Int(i64::MAX),
        FilterValue::Float(0.0),
        FilterValue::Float(-1.5),
        FilterValue::String(String::new()),
        FilterValue::String("round-trip".to_string()),
        FilterValue::Binary(vec![]),
        FilterValue::Binary(vec![0xde, 0xad, 0xbe, 0xef]),
        FilterValue::Array(vec![FilterValue::Int(1), FilterValue::Bool(false)]),
    ];

    for fv in &literals {
        let qv = filter_value_to_query_value(fv)
            .unwrap_or_else(|| panic!("filter_value_to_query_value returned None for {:?}", fv));
        let back = query_value_to_filter_value(&qv)
            .unwrap_or_else(|| panic!("query_value_to_filter_value returned None for {:?}", qv));
        assert_eq!(&back, fv, "round-trip failed for {:?}", fv);
    }
}

// ── From<QueryValue> for FilterValue ────────────────────────────────────────

#[test]
fn from_qv_literal_is_direct_not_null() {
    // Regression: before this fix, From<QueryValue> used msgpack+unwrap_or(Null).
    // A valid literal must NOT become Null.
    let cases = vec![
        (QueryValue::Bool(true), FilterValue::Bool(true)),
        (QueryValue::Int(99), FilterValue::Int(99)),
        (
            QueryValue::Str("abc".to_string()),
            FilterValue::String("abc".to_string()),
        ),
        (QueryValue::Null, FilterValue::Null),
    ];
    for (qv, expected) in cases {
        let got = FilterValue::from(qv.clone());
        assert_eq!(
            got, expected,
            "From<QueryValue>({:?}) gave wrong result",
            qv
        );
    }
}

#[test]
fn from_qv_list_converts_recursively() {
    let qv = QueryValue::List(vec![QueryValue::Int(5), QueryValue::Bool(true)]);
    let got = FilterValue::from(qv);
    assert_eq!(
        got,
        FilterValue::Array(vec![FilterValue::Int(5), FilterValue::Bool(true)])
    );
}
