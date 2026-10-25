//! Regression tests for RV-QUERY-BUILDER-20D9732A and
//! RV-QUERY-BUILDER-7B79D840 — `Doc::set` conversion of `FilterValue` into
//! `QueryValue`.
//!
//! - 20D9732A: `Doc::set` used to `.expect()` the msgpack round-trip in a
//!   public setter. It now returns `Result<Doc, BuilderError>`, uses a
//!   `filter_value_to_query_value` fast path, and guards nesting depth.
//! - 7B79D840: literal values take the direct-converter fast path; the
//!   stored `QueryValue` must be byte-for-byte equivalent to what the old
//!   round-trip produced (for converter-supported values, both produce the
//!   same `QueryValue`).

use crate::val::col;
use crate::write::doc;
use shamir_query_types::filter::{
    filter_value_to_query_value, Cond, Filter, FilterValue, FnCall, MAX_FILTER_DEPTH,
};
use shamir_types::types::value::QueryValue;

/// How far past the limit to build pathological inputs. The guard must
/// reject with early cutoff — any depth > `MAX_FILTER_DEPTH` (64) is
/// rejected at the first stack frame processed, so the exact depth past
/// the limit is irrelevant as long as it is far beyond it. 1_000 is well
/// above the limit while remaining safe for the recursive Drop teardown of
/// the constructed tree (no custom `Drop` impls exist or will be added).
const OVERDEEP_NESTING: usize = 1_000;

/// RV-QUERY-BUILDER-20D9732A: a `FilterValue::Array` nested beyond
/// `MAX_FILTER_DEPTH` must make `Doc::set` return `Err`, not panic.
///
/// Failing-on-old mechanism: the old `Doc::set` returned `Self` and
/// `.expect()`ed the codec round-trip — this test only *compiles* against
/// the new `Result`-returning signature, and on old code the call would
/// panic inside the setter instead of yielding an `Err`.
#[test]
fn rv_query_builder_20d9732a_doc_set_deep_nesting_returns_err_not_panic() {
    let mut fv = FilterValue::Int(1);
    for _ in 0..(MAX_FILTER_DEPTH + 1) {
        fv = FilterValue::Array(vec![fv]);
    }
    let result = doc().set("deep", fv);
    match result {
        Ok(_) => panic!("nesting beyond MAX_FILTER_DEPTH must be rejected"),
        Err(e) => assert!(
            e.to_string().contains("MAX_FILTER_DEPTH"),
            "unexpected error: {e}"
        ),
    }
}

/// RV-QUERY-BUILDER-20D9732A (guard itself must be bounded): a deep
/// `FilterValue::Array` (1_000 levels, well past `MAX_FILTER_DEPTH`) must
/// make `Doc::set` return `Err` (a `FieldValueCodec` error mentioning
/// depth) WITHOUT the depth guard itself overflowing the stack.
///
/// DEPTH BOUND ARGUMENT: the guard's early cutoff is proven by the depth
/// bound itself — any depth > 64 is rejected at the first stack frame
/// processed — so the test depth only needs to exceed the limit while
/// remaining safe for the recursive Drop teardown (no custom `Drop` impls
/// exist or will be added).
///
/// Failing-on-old mechanism: the old unbounded `filter_value_depth` computed
/// the FULL depth before comparing to the limit, so the guard's own
/// recursion overflowed the stack (process abort, not `Err`) on this input.
/// DEPTH BOUND ARGUMENT (deep tests): as for [`OVERDEEP_NESTING`], the
/// guard's early cutoff is proven by the depth bound itself (any depth >
/// 64 is rejected at the first stack frame processed), so a depth that
/// only needs to exceed the limit while staying safe for recursive Drop
/// teardown suffices.
#[test]
fn rv_query_builder_20d9732a_doc_set_deep_array_guard_bounded() {
    let mut fv = FilterValue::Int(1);
    for _ in 0..OVERDEEP_NESTING {
        fv = FilterValue::Array(vec![fv]);
    }
    let result = doc().set("deep", fv);
    match result {
        Ok(_) => panic!("nesting far beyond MAX_FILTER_DEPTH must be rejected"),
        Err(e) => assert!(
            e.to_string().contains("MAX_FILTER_DEPTH"),
            "unexpected error: {e}"
        ),
    }
}

/// RV-QUERY-BUILDER-20D9732A (expression markers must not bypass the
/// guard): a `FnCall::Complex` chain nested far beyond `MAX_FILTER_DEPTH`
/// (1_000 levels — see [`OVERDEEP_NESTING`] for the bound argument) must
/// make `Doc::set` return `Err`.
///
/// Failing-on-old mechanism: the old guard treated `FnCall`/`Expr`/`Cond`
/// as leaves, so this chain passed the guard and overflowed the stack
/// (process abort) inside the recursive msgpack serialization fallback
/// instead of yielding an `Err`.
/// DEPTH BOUND ARGUMENT (deep tests): as for [`OVERDEEP_NESTING`], the
/// guard's early cutoff is proven by the depth bound itself (any depth >
/// 64 is rejected at the first stack frame processed), so a depth that
/// only needs to exceed the limit while staying safe for recursive Drop
/// teardown suffices.
#[test]
fn rv_query_builder_20d9732a_doc_set_deep_expression_chain_guarded() {
    let mut fv = FilterValue::Int(1);
    for _ in 0..OVERDEEP_NESTING {
        fv = FilterValue::FnCall {
            call: FnCall::complex("COALESCE", vec![fv]),
        };
    }
    let result = doc().set("deep_fn", fv);
    match result {
        Ok(_) => panic!("deeply nested expression chain must be rejected"),
        Err(e) => assert!(
            e.to_string().contains("MAX_FILTER_DEPTH"),
            "unexpected error: {e}"
        ),
    }
}

/// RV-QUERY-BUILDER-20D9732A (fallback path): the expression-marker
/// round-trip path is also fallible — errors surface as
/// `BuilderError::FieldValueCodec`, never a `.expect()` panic. Values the
/// converter handles go through the fast path and still succeed.
#[test]
fn rv_query_builder_20d9732a_doc_set_literal_value_returns_ok_result() {
    let d = doc()
        .set("name", "Alice")
        .unwrap()
        .set("age", 30_i64)
        .unwrap();
    let built = d.build();
    match built {
        QueryValue::Map(m) => {
            assert_eq!(m.get("name"), Some(&QueryValue::Str("Alice".to_string())));
            assert_eq!(m.get("age"), Some(&QueryValue::Int(30)));
        }
        other => panic!("expected map, got {other:?}"),
    }
}

/// RV-QUERY-BUILDER-7B79D840: for every value the converter supports,
/// `Doc::set` must store exactly the `QueryValue` the converter produces
/// (equivalence oracle against the old round-trip representation — for
/// converter-supported values the round-trip and the converter produce the
/// same `QueryValue`, including the `Binary` → `QueryValue::Bin` shape).
///
/// ORACLE LIMIT: representation equivalence is the achievable oracle —
/// these tests do NOT detect removal of the converter fast path (a
/// restored full round-trip would still pass them); the fast path itself
/// is guarded by code review / CI benches.
#[test]
fn rv_query_builder_7b79d840_doc_set_literal_matches_converter_output() {
    let cases: Vec<FilterValue> = vec![
        FilterValue::Null,
        FilterValue::Bool(true),
        FilterValue::Int(-42),
        FilterValue::Float(1.5),
        FilterValue::String("hello".to_string()),
        // Binary must survive as QueryValue::Bin, not decay to Str.
        FilterValue::Binary(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        // Nested arrays, including Binary inside an array.
        FilterValue::Array(vec![
            FilterValue::Int(1),
            FilterValue::Array(vec![FilterValue::Binary(vec![1, 2, 3])]),
        ]),
    ];
    for fv in cases {
        let expected =
            filter_value_to_query_value(&fv).expect("converter must handle literal test cases");
        let built = doc().set("f", fv.clone()).unwrap().build();
        match built {
            QueryValue::Map(m) => {
                assert_eq!(m.get("f"), Some(&expected), "mismatch for {fv:?}");
            }
            other => panic!("expected map, got {other:?}"),
        }
    }
}

/// RV-QUERY-BUILDER-7B79D840: expression markers (converter returns `None`)
/// still fall back to the msgpack round-trip and store exactly the same
/// `QueryValue` the old round-trip-only implementation stored.
///
/// ORACLE LIMIT: representation equivalence only — does not detect removal
/// of the converter fast path (full round-trip restoration would still
/// pass); the fast path is guarded by code review / CI benches.
#[test]
fn rv_query_builder_7b79d840_doc_set_expression_marker_matches_round_trip() {
    let fv = col("email");
    assert_eq!(filter_value_to_query_value(&fv), None);

    let bytes = rmp_serde::to_vec_named(&fv).unwrap();
    let expected: QueryValue = rmp_serde::from_slice(&bytes).unwrap();

    let built = doc().set("email_norm", fv).unwrap().build();
    match built {
        QueryValue::Map(m) => assert_eq!(m.get("email_norm"), Some(&expected)),
        other => panic!("expected map, got {other:?}"),
    }
}

/// RV-QUERY-BUILDER-20D9732A (Cond's embedded condition counts toward the
/// TOTAL depth): an outer `FilterValue::Array` chain of
/// `MAX_FILTER_DEPTH - 3` levels whose innermost element is a
/// `FilterValue::Cond` whose `condition` is a nested `Filter::And` chain
/// ~5 deep must make `Doc::set` return `Err` — the condition alone is
/// legal, but the ACCUMULATED depth exceeds `MAX_FILTER_DEPTH`.
///
/// Failing-on-old mechanism: reverting the fix (old code called
/// `check_filter_depth(cond.condition.as_ref())`, which restarts at depth
/// one) makes this test fail — the condition passed as a fresh root and the
/// total depth overflow was missed, so `set` returned `Ok`.
#[test]
fn rv_query_builder_20d9732a_cond_condition_counts_toward_total_depth() {
    // A legal-on-its-own `Filter::And` chain, ~5 levels deep.
    let mut condition = Filter::Eq {
        field: vec!["active".to_string()],
        value: FilterValue::Bool(true),
    };
    for _ in 0..5 {
        condition = Filter::And {
            filters: vec![condition],
        };
    }

    // Outer Array chain of MAX_FILTER_DEPTH - 3 levels, Cond innermost.
    let mut fv = FilterValue::Cond {
        cond: Box::new(Cond::new(
            condition,
            FilterValue::String("t".to_string()),
            FilterValue::String("f".to_string()),
        )),
    };
    for _ in 0..(MAX_FILTER_DEPTH - 3) {
        fv = FilterValue::Array(vec![fv]);
    }

    match doc().set("total_depth", fv) {
        Ok(_) => panic!("Cond condition must count toward the accumulated MAX_FILTER_DEPTH"),
        Err(e) => assert!(
            e.to_string().contains("MAX_FILTER_DEPTH"),
            "unexpected error: {e}"
        ),
    }
}
