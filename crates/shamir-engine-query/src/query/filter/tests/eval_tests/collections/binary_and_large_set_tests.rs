//! Collection-membership filter tests: In/NotIn over literals, QueryRef
//! columns, InSet and $contains variants.

use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use super::numeric_coercion_tests::make_int_field_record;
use crate::query::filter::tests::eval_tests::shared::helpers::{empty_refs, make_alice_record};

// F-21 group regressions — per-row allocation elimination in filter eval.
//
// These pin CORRECTNESS of the zero-/reduced-allocation rewrites (not perf
// itself): the borrow-based `$in`/`InSet` binary probe, the bitmask-based
// `$contains_all` scan (incl. the >64-required-values `Vec<bool>` fallback
// and the exact-64 `u64::MAX` boundary), and the `$contains` fast path that
// skips `materialize_at` for non-container fields.
// ============================================================================

fn make_bin_field_record(interner: &Interner, field: &str, bytes: &[u8]) -> InnerValue {
    let mut map = new_map();
    let k = interner.touch_ind(field).unwrap().into_key();
    map.insert(k, InnerValue::Bin(bytes.to_vec()));
    InnerValue::Map(map)
}

#[test]
fn test_inset_binary_field_match() {
    // InSet's Bin probe now hashes/compares a borrowed `&[u8]` (BinProbe)
    // instead of allocating an owned `QueryValue::Bin(Vec<u8>)` per row.
    let interner = Interner::new();
    let record = make_bin_field_record(&interner, "payload", &[1, 2, 3]);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["payload".to_string()],
        values: vec![
            FilterValue::Binary(vec![1, 2, 3]),
            FilterValue::Binary(vec![9, 9, 9]),
        ],
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        matches!(
            node,
            crate::query::filter::filter_node::FilterNode::InSet { .. }
        ),
        "all-literal $in must compile to InSet"
    );
    assert!(
        node.matches(&record, &ctx),
        "Bin([1,2,3]) must match $in [[1,2,3],[9,9,9]]"
    );
}

#[test]
fn test_inset_binary_field_no_match() {
    let interner = Interner::new();
    let record = make_bin_field_record(&interner, "payload", &[1, 2, 3]);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["payload".to_string()],
        values: vec![FilterValue::Binary(vec![9, 9, 9])],
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        !node.matches(&record, &ctx),
        "Bin([1,2,3]) must NOT match $in [[9,9,9]]"
    );
}

#[test]
fn test_notin_binary_field() {
    let interner = Interner::new();
    let record = make_bin_field_record(&interner, "payload", &[1, 2, 3]);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotIn {
        field: vec!["payload".to_string()],
        values: vec![FilterValue::Binary(vec![9, 9, 9])],
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        node.matches(&record, &ctx),
        "Bin([1,2,3]) must match $nin [[9,9,9]]"
    );
}

/// Build a record `{field: [v0, v1, ..., v(n-1)]}` (List of distinct strings).
fn make_string_list_record(interner: &Interner, field: &str, n: usize) -> InnerValue {
    let mut map = new_map();
    let k = interner.touch_ind(field).unwrap().into_key();
    map.insert(
        k,
        InnerValue::List((0..n).map(|i| InnerValue::Str(format!("v{i}"))).collect()),
    );
    InnerValue::Map(map)
}

fn string_values(n: usize) -> Vec<FilterValue> {
    (0..n)
        .map(|i| FilterValue::String(format!("v{i}")))
        .collect()
}

#[test]
fn test_contains_all_set_over_64_values_all_present() {
    // 70 required values (> 64) forces ContainsAllSet's `Vec<bool>` fallback
    // path (the u64 bitmask only covers <=64). Field contains every required
    // value plus one extra — must match.
    let interner = Interner::new();
    let record = make_string_list_record(&interner, "tags", 71); // v0..v70
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAll {
        field: vec!["tags".to_string()],
        values: string_values(70), // v0..v69
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        matches!(
            node,
            crate::query::filter::filter_node::FilterNode::ContainsAllSet { .. }
        ),
        "all-literal $contains_all must compile to ContainsAllSet"
    );
    assert!(
        node.matches(&record, &ctx),
        "field superset of 70 required values must match $contains_all"
    );
}

#[test]
fn test_contains_all_set_over_64_values_one_missing() {
    // Same 70-required-value shape, but the field is missing exactly one
    // required value — must NOT match. Exercises the `Vec<bool>` fallback's
    // "remaining never reaches 0" path.
    let interner = Interner::new();
    // Field has v0..v68 (69 values) — missing "v69".
    let record = make_string_list_record(&interner, "tags", 69);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAll {
        field: vec!["tags".to_string()],
        values: string_values(70), // v0..v69, "v69" is absent from the field
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        !node.matches(&record, &ctx),
        "field missing one of 70 required values must NOT match $contains_all"
    );
}

#[test]
fn test_contains_all_set_exact_64_boundary() {
    // n == 64 exercises the special-cased `target = u64::MAX` branch (shifting
    // `1u64 << 64` would be UB/panic, hence the special case).
    let interner = Interner::new();
    let record = make_string_list_record(&interner, "tags", 64); // v0..v63, exact match
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAll {
        field: vec!["tags".to_string()],
        values: string_values(64),
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        node.matches(&record, &ctx),
        "exact 64-value field/required-set match must pass $contains_all"
    );

    // Now drop one field value — must fail.
    let record_missing = make_string_list_record(&interner, "tags", 63); // v0..v62
    assert!(
        !node.matches(&record_missing, &ctx),
        "63-of-64 required values present must NOT match $contains_all"
    );
}

#[test]
fn test_contains_against_int_field_is_false() {
    // `$contains` against a plain scalar Int field must be false WITHOUT
    // walking the container-materialize path (present_kind_at reports
    // Scalar, not Container, so the fast path returns directly).
    let interner = Interner::new();
    let record = make_int_field_record(&interner, "n", 42);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Contains {
        field: vec!["n".to_string()],
        value: FilterValue::String("4".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_contains_against_null_field_is_false() {
    let interner = Interner::new();
    let mut map = new_map();
    let k = interner.touch_ind("n").unwrap().into_key();
    map.insert(k, InnerValue::Null);
    let record = InnerValue::Map(map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Contains {
        field: vec!["n".to_string()],
        value: FilterValue::String("x".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_contains_against_absent_field_is_false() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // has no "missing_field"
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Contains {
        field: vec!["missing_field".to_string()],
        value: FilterValue::String("x".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}
