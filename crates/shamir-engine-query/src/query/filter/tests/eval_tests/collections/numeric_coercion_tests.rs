//! Collection-membership filter tests: In/NotIn over literals, QueryRef
//! columns, InSet and $contains variants.

use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::filter_node::FilterNode;
use crate::query::filter::{Filter, FilterValue};
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::value::{InnerValue, QueryValue};

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

#[test]
fn contains_all_literal_dynamic_numeric_parity_both_masks() {
    for extra in [0, 63] {
        for (item, literal, expected) in [
            (InnerValue::Int(1), FilterValue::Float(1.0), true),
            (InnerValue::F64(1.0), FilterValue::Float(1.0), true),
            (
                InnerValue::Big(BigInt::from(1)),
                FilterValue::Float(1.0),
                true,
            ),
            (
                InnerValue::Dec(Decimal::from(1)),
                FilterValue::Float(1.0),
                true,
            ),
            (
                InnerValue::F64(f64::NAN),
                FilterValue::Float(f64::NAN),
                false,
            ),
            (InnerValue::Int(2), FilterValue::Float(1.0), false),
        ] {
            let interner = Interner::new();
            let mut map = new_map();
            let mut items = vec![item];
            let mut required = vec![FilterValue::Int(1), literal];
            for n in 10..10 + extra {
                items.push(InnerValue::Int(n));
                required.push(FilterValue::Int(n));
            }
            map.insert(
                interner.touch_ind("items").unwrap().into_key(),
                InnerValue::List(items),
            );
            map.insert(
                interner.touch_ind("one").unwrap().into_key(),
                InnerValue::Int(1),
            );
            let record = InnerValue::Map(map);
            let refs = empty_refs();
            let ctx = FilterContext::new(&interner, &refs);
            let literal_filter = Filter::ContainsAll {
                field: vec!["items".to_string()],
                values: required.clone(),
            };
            required[0] = FilterValue::FieldRef {
                path: vec!["one".to_string()],
            };
            let dynamic_filter = Filter::ContainsAll {
                field: vec!["items".to_string()],
                values: required,
            };
            let dynamic = compile_filter(&dynamic_filter, &interner).matches(&record, &ctx);
            let optimized = compile_filter(&literal_filter, &interner).matches(&record, &ctx);
            assert_eq!(dynamic, expected);
            assert_eq!(optimized, dynamic, "extra requirements: {extra}");
        }
    }
}

#[test]
fn contains_all_literal_dynamic_nonnumeric_and_numeric_boundaries() {
    for (item, literal, expected) in [
        (
            InnerValue::Str("alpha".into()),
            FilterValue::String("alpha".into()),
            true,
        ),
        (
            InnerValue::Str("alpha".into()),
            FilterValue::String("beta".into()),
            false,
        ),
        (InnerValue::F64(-0.0), FilterValue::Int(0), true),
        (
            InnerValue::Int(9_007_199_254_740_993),
            FilterValue::Float(9_007_199_254_740_992.0),
            false,
        ),
        (
            InnerValue::F64(f64::INFINITY),
            FilterValue::Float(f64::INFINITY),
            true,
        ),
    ] {
        let interner = Interner::new();
        let mut map = new_map();
        map.insert(
            interner.touch_ind("items").unwrap().into_key(),
            InnerValue::List(vec![item]),
        );
        let other = match &literal {
            FilterValue::Int(n) => InnerValue::Int(*n),
            FilterValue::Float(n) => InnerValue::F64(*n),
            FilterValue::String(s) => InnerValue::Str(s.clone()),
            _ => unreachable!(),
        };
        map.insert(interner.touch_ind("other").unwrap().into_key(), other);
        let record = InnerValue::Map(map);
        let refs = empty_refs();
        let ctx = FilterContext::new(&interner, &refs);
        let fast = Filter::ContainsAll {
            field: vec!["items".into()],
            values: vec![literal],
        };
        let dynamic = Filter::ContainsAll {
            field: vec!["items".into()],
            values: vec![FilterValue::FieldRef {
                path: vec!["other".into()],
            }],
        };
        assert_eq!(
            compile_filter(&dynamic, &interner).matches(&record, &ctx),
            expected
        );
        assert_eq!(
            compile_filter(&fast, &interner).matches(&record, &ctx),
            expected
        );
    }
}

#[test]
fn public_contains_all_set_matches_big_and_decimal_requirements() {
    for extra in [0, 63] {
        for item in [InnerValue::Int(1), InnerValue::F64(1.0)] {
            let interner = Interner::new();
            let key = interner.touch_ind("items").unwrap().into_key();
            let mut values = new_set();
            values.insert(QueryValue::Int(1));
            values.insert(QueryValue::F64(1.0));
            values.insert(QueryValue::Big(BigInt::from(1)));
            values.insert(QueryValue::Dec(Decimal::from(1)));
            let mut items = vec![item];
            for n in 10..10 + extra {
                values.insert(QueryValue::Int(n));
                items.push(InnerValue::Int(n));
            }
            let node = FilterNode::ContainsAllSet {
                field_path: [key.clone()].into_iter().collect(),
                values,
            };
            let mut record = new_map();
            record.insert(key, InnerValue::List(items));
            let refs = empty_refs();
            let ctx = FilterContext::new(&interner, &refs);
            assert!(node.matches(&InnerValue::Map(record), &ctx));
        }
    }
}

#[test]
fn public_contains_all_set_rejects_container_requirement() {
    let interner = Interner::new();
    let key = interner.touch_ind("items").unwrap().into_key();
    let mut values = new_set();
    values.insert(QueryValue::List(vec![QueryValue::Int(1)]));
    let node = FilterNode::ContainsAllSet {
        field_path: [key.clone()].into_iter().collect(),
        values,
    };
    let mut record = new_map();
    record.insert(
        key,
        InnerValue::List(vec![InnerValue::List(vec![InnerValue::Int(1)])]),
    );
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    assert!(!node.matches(&InnerValue::Map(record), &ctx));
}
