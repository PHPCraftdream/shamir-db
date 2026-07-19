//! Unit tests for [`filter_value_to_query_value`] (the record-free
//! `Call`-param positional resolver).
//!
//! These tests assert the observable *behavior* of the resolver:
//!
//! - Literals and `$query` refs resolve correctly (regression — these were
//!   never part of the collapse).
//! - Dynamic markers (`$ref` / `$fn` / `$expr` / `$cond`) still collapse to
//!   `Null` (fail-open preserved) — the only change in the audit fix is that
//!   the collapse is now also surfaced via a `warn!` log line.
//!
//! The `warn!` log itself is verified by code inspection rather than
//! captured: there is no log-capture test harness in the `shamir-db` crate
//! (the `tracing_subscriber`-based capture lives only in `shamir-server`).
//! Inventing one for a single warning is explicitly out of scope for this
//! task.

use shamir_query_types::filter::FnCall;
use shamir_query_types::read::QueryResult;
use shamir_types::types::value::QueryValue;

use crate::query::FilterValue;
use crate::types::common::TMap;

use super::super::helpers::filter_value_to_query_value;

// ── literals (regression: never part of the collapse) ────────────────────────

#[test]
fn literal_int_resolves_unchanged() {
    let refs = TMap::default();
    let qv = filter_value_to_query_value(&FilterValue::Int(7), &refs);
    assert_eq!(qv, QueryValue::Int(7));
}

#[test]
fn literal_string_resolves_unchanged() {
    let refs = TMap::default();
    let qv = filter_value_to_query_value(&FilterValue::String("hi".into()), &refs);
    assert_eq!(qv, QueryValue::Str("hi".into()));
}

#[test]
fn array_of_literals_resolves_elementwise() {
    // Arrays recurse through the resolver — literals pass through untouched.
    let arr = FilterValue::Array(vec![FilterValue::Int(1), FilterValue::Int(2)]);
    let refs = TMap::default();
    let qv = filter_value_to_query_value(&arr, &refs);
    assert_eq!(
        qv,
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Int(2)])
    );
}

// ── $query / QueryRef (regression: resolves against resolved_refs) ───────────

#[test]
fn query_ref_resolves_value_from_resolved_refs() {
    // A Call result lives in `value` (value-first rule).
    let mut refs: TMap<String, QueryResult> = TMap::default();
    refs.insert(
        "q1".to_string(),
        QueryResult {
            records: vec![],
            stats: None,
            pagination: None,
            value: Some(QueryValue::Int(42)),
            explain: None,
            skipped: false,
        },
    );

    let qv = filter_value_to_query_value(&FilterValue::query_ref("q1"), &refs);
    assert_eq!(qv, QueryValue::Int(42));
}

#[test]
fn query_ref_missing_alias_collapses_to_null() {
    // No entry under the alias → Null (pre-existing behavior, unchanged).
    let refs: TMap<String, QueryResult> = TMap::default();
    let qv = filter_value_to_query_value(&FilterValue::query_ref("nope"), &refs);
    assert_eq!(qv, QueryValue::Null);
}

// ── dynamic markers (collapse to Null preserved; now also warn-logged) ───────
//
// Each of these still resolves to `Null` (the fail-open/collapse behavior is
// UNCHANGED by this fix).  The fix adds a `warn!` log naming the marker; the
// log is confirmed by code inspection (see module doc-comment above).

#[test]
fn field_ref_collapses_to_null() {
    // $ref — no record context to resolve a field path against here.
    let refs = TMap::default();
    let qv = filter_value_to_query_value(&FilterValue::field_ref("some_field"), &refs);
    assert_eq!(qv, QueryValue::Null);
}

#[test]
fn fn_call_collapses_to_null() {
    // $fn — not meaningful as a positional param here.
    let refs = TMap::default();
    let fv = FilterValue::FnCall {
        call: FnCall::simple("NOW"),
    };
    let qv = filter_value_to_query_value(&fv, &refs);
    assert_eq!(qv, QueryValue::Null);
}

#[test]
fn array_with_nested_dynamic_marker_collapses_element_to_null() {
    // Recursion edge case: a dynamic marker nested inside an Array collapses
    // to Null element-wise — the surrounding list is still built.
    let arr = FilterValue::Array(vec![FilterValue::Int(1), FilterValue::field_ref("missing")]);
    let refs = TMap::default();
    let qv = filter_value_to_query_value(&arr, &refs);
    assert_eq!(
        qv,
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Null])
    );
}
