//! Collection-membership filter tests: In/NotIn over literals, QueryRef
//! columns, InSet and $contains variants.

use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::value::InnerValue;

use crate::query::filter::tests::eval_tests::shared::helpers::empty_refs;
// ============================================================================
// Int↔F64 coercion in all-literal fast-path nodes (InSet/ContainsAnySet/ContainsAllSet)
//
// The slow-path nodes (In, ContainsAny, ContainsAll) go through
// scalar_ref_cmp_qv / compare_values which treat Int(1) and F64(1.0) as equal.
// The all-literal fast-path nodes previously used exact TSet::contains /
// swap_remove, so the same logical filter gave different answers depending on
// whether its value list happened to be fully literal.
// ============================================================================

pub(in crate::query::filter::tests::eval_tests::collections) fn make_int_field_record(
    interner: &Interner,
    field: &str,
    val: i64,
) -> InnerValue {
    let mut map = new_map();
    let k = interner.touch_ind(field).unwrap().into_key();
    map.insert(k, InnerValue::Int(val));
    InnerValue::Map(map)
}

pub(in crate::query::filter::tests::eval_tests::collections) fn make_f64_field_record(
    interner: &Interner,
    field: &str,
    val: f64,
) -> InnerValue {
    let mut map = new_map();
    let k = interner.touch_ind(field).unwrap().into_key();
    map.insert(k, InnerValue::F64(val));
    InnerValue::Map(map)
}

#[test]
fn test_inset_int_field_f64_literal_coerces() {
    // InSet: field n = Int(1), filter {$in: [1.0]} — must MATCH.
    let interner = Interner::new();
    let record = make_int_field_record(&interner, "n", 1);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["n".to_string()],
        values: vec![FilterValue::Float(1.0)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        cb.matches(&record, &ctx),
        "Int(1) field must match $in [1.0]"
    );
}

#[test]
fn test_inset_f64_field_int_literal_coerces() {
    // InSet: field n = F64(1.0), filter {$in: [1]} — must MATCH.
    let interner = Interner::new();
    let record = make_f64_field_record(&interner, "n", 1.0);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["n".to_string()],
        values: vec![FilterValue::Int(1)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        cb.matches(&record, &ctx),
        "F64(1.0) field must match $in [1]"
    );
}

#[test]
fn test_inset_no_false_positive_on_non_integer_f64() {
    // InSet: field n = Int(1), filter {$in: [1.5]} — must NOT match.
    let interner = Interner::new();
    let record = make_int_field_record(&interner, "n", 1);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["n".to_string()],
        values: vec![FilterValue::Float(1.5)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        !cb.matches(&record, &ctx),
        "Int(1) must NOT match $in [1.5]"
    );
}

#[test]
fn test_contains_any_set_int_list_f64_literal_coerces() {
    // ContainsAnySet: field is List containing Int(1), required {1.0} — MATCH.
    let interner = Interner::new();
    let k = interner.touch_ind("items").unwrap().into_key();
    let mut map = new_map();
    map.insert(
        k,
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
    );
    let record = InnerValue::Map(map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAny {
        field: vec!["items".to_string()],
        values: vec![FilterValue::Float(1.0)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        cb.matches(&record, &ctx),
        "List[Int(1),Int(2)] must match $contains_any [1.0]"
    );
}

#[test]
fn test_contains_any_set_int_set_f64_literal_coerces() {
    // ContainsAnySet: field is Set containing Int(1), required {1.0} — MATCH.
    let interner = Interner::new();
    let k = interner.touch_ind("items").unwrap().into_key();
    let mut s = new_set();
    s.insert(InnerValue::Int(1));
    s.insert(InnerValue::Int(2));
    let mut map = new_map();
    map.insert(k, InnerValue::Set(s));
    let record = InnerValue::Map(map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAny {
        field: vec!["items".to_string()],
        values: vec![FilterValue::Float(1.0)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        cb.matches(&record, &ctx),
        "Set{{Int(1),Int(2)}} must match $contains_any [1.0]"
    );
}

#[test]
fn test_contains_all_set_int_list_f64_literals_coerces() {
    // ContainsAllSet: field contains [Int(1), Int(2)], required {1.0, 2.0} — MATCH.
    // Exercises the swap_remove-must-actually-remove coercion path.
    let interner = Interner::new();
    let k = interner.touch_ind("items").unwrap().into_key();
    let mut map = new_map();
    map.insert(
        k,
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
    );
    let record = InnerValue::Map(map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAll {
        field: vec!["items".to_string()],
        values: vec![FilterValue::Float(1.0), FilterValue::Float(2.0)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        cb.matches(&record, &ctx),
        "List[Int(1),Int(2)] must match $contains_all [1.0, 2.0]"
    );
}

#[test]
fn test_contains_all_set_int_list_f64_literals_partial_no_false_positive() {
    // ContainsAllSet: field contains [Int(1), Int(2)], required {1.0, 3.0} —
    // must NOT match (3 absent). Verifies the coercing swap_remove correctly
    // tracks remaining count.
    let interner = Interner::new();
    let k = interner.touch_ind("items").unwrap().into_key();
    let mut map = new_map();
    map.insert(
        k,
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
    );
    let record = InnerValue::Map(map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAll {
        field: vec!["items".to_string()],
        values: vec![FilterValue::Float(1.0), FilterValue::Float(3.0)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        !cb.matches(&record, &ctx),
        "List[Int(1),Int(2)] must NOT match $contains_all [1.0, 3.0]"
    );
}

#[test]
fn test_inset_non_numeric_exact_match_unchanged() {
    // Regression: non-numeric types continue to use exact matching — no
    // accidental coercion introduced for Str/Bool.
    let interner = Interner::new();
    let mut map = new_map();
    let k = interner.touch_ind("s").unwrap().into_key();
    map.insert(k, InnerValue::Str("hello".to_string()));
    let record = InnerValue::Map(map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // Exact match
    let f1 = Filter::In {
        field: vec!["s".to_string()],
        values: vec![FilterValue::String("hello".to_string())],
    };
    assert!(compile_filter(&f1, &interner).matches(&record, &ctx));

    // No false cross-type match
    let f2 = Filter::In {
        field: vec!["s".to_string()],
        values: vec![FilterValue::String("world".to_string())],
    };
    assert!(!compile_filter(&f2, &interner).matches(&record, &ctx));
}
