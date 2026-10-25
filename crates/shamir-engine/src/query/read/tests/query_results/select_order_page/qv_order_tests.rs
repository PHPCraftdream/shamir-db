//! Self-contained tests for QueryValue post-processors.
//!
//! Each test asserts that the QueryValue-based post-processor
//! (distinct_qv, order_by_qv, pagination<T>) produces the correct
//! result against explicit expected values, including the Dec/Big/Bin/Set
//! edge cases where canonical-key mapping is essential for correct dedup.
//!
//! The old parity checks (comparing against the now-deleted
//! apply_distinct / apply_select legacy path) have been replaced with
//! concrete assertions against known-correct QueryValue results.

use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

use crate::query::read::exec::{apply_distinct_qv, apply_pagination};
use crate::query::read::order::apply_order_by_qv;
use crate::query::read::{NullsOrder, OrderBy, OrderByItem, OrderDirection, Pagination};

/// Helper: build a QueryValue map from key-value pairs.
pub(in crate::query::read::tests) fn qv_map(pairs: &[(&str, QueryValue)]) -> QueryValue {
    let mut m = new_map_wc(pairs.len());
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    QueryValue::Map(m)
}

// ============================================================================
// Pagination (generic over T)
// ============================================================================

#[test]
fn pagination_qv_limit_offset() {
    let qvs: Vec<QueryValue> = (1..=5).map(QueryValue::Int).collect();

    let pag = Pagination::LimitOffset {
        limit: Some(2),
        offset: 1,
    };

    let (result, info) = apply_pagination(qvs, &pag, true);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0], QueryValue::Int(2));
    assert_eq!(result[1], QueryValue::Int(3));
    let info = info.unwrap();
    assert_eq!(info.total_count, Some(5));
    assert!(info.has_next);
    assert!(info.has_prev);
}

#[test]
fn pagination_qv_page_based() {
    let qvs: Vec<QueryValue> = (1..=5).map(QueryValue::Int).collect();

    let pag = Pagination::page(2, 2);
    let (result, info) = apply_pagination(qvs, &pag, true);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0], QueryValue::Int(3));
    assert_eq!(result[1], QueryValue::Int(4));
    let info = info.unwrap();
    assert_eq!(info.total_count, Some(5));
    assert_eq!(info.current_page, Some(2));
    assert!(info.has_next);
    assert!(info.has_prev);
}

// ============================================================================
// Distinct (Stage B)
// ============================================================================

#[test]
fn distinct_qv_scalar_duplicates() {
    let qvs = vec![
        qv_map(&[("a", QueryValue::Int(1))]),
        qv_map(&[("a", QueryValue::Int(2))]),
        qv_map(&[("a", QueryValue::Int(1))]),
        qv_map(&[("a", QueryValue::Int(3))]),
        qv_map(&[("a", QueryValue::Int(2))]),
    ];

    let result = apply_distinct_qv(qvs);

    // 3 distinct values, insertion-order preserved
    assert_eq!(result.len(), 3);
    assert_eq!(result[0]["a"], QueryValue::Int(1));
    assert_eq!(result[1]["a"], QueryValue::Int(2));
    assert_eq!(result[2]["a"], QueryValue::Int(3));
}

#[test]
fn distinct_qv_dec_vs_str_separate_classes() {
    // D1-S2: Dec and its decimal string are separate DISTINCT classes (a Dec
    // is an exact numeric class; strings never alias numbers).
    let qvs = vec![
        qv_map(&[("v", QueryValue::Dec("1.0".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Str("1.0".to_string()))]),
        qv_map(&[("v", QueryValue::Int(2))]),
    ];

    let result = apply_distinct_qv(qvs);

    assert_eq!(result.len(), 3, "Dec and Str form must not dedup");
    assert!(matches!(&result[0]["v"], QueryValue::Dec(_)));
    assert!(matches!(&result[1]["v"], QueryValue::Str(_)));
    assert_eq!(result[2]["v"], QueryValue::Int(2));
}

#[test]
fn distinct_qv_big_vs_str_separate_classes() {
    use num_bigint::BigInt;
    let qvs = vec![
        qv_map(&[("v", QueryValue::Big(BigInt::from(42)))]),
        qv_map(&[("v", QueryValue::Str("42".to_string()))]),
        qv_map(&[("v", QueryValue::Int(99))]),
    ];

    let result = apply_distinct_qv(qvs);

    // D1-S2: Big(42) is an exact numeric class; the string "42" is not.
    assert_eq!(result.len(), 3, "Big and Str form must not dedup");
    assert!(matches!(&result[0]["v"], QueryValue::Big(_)));
    assert!(matches!(&result[1]["v"], QueryValue::Str(_)));
    assert_eq!(result[2]["v"], QueryValue::Int(99));
}

#[test]
fn distinct_qv_bin_dedup() {
    // Two identical Bin values should deduplicate; a unique Bin is kept.
    let qvs = vec![
        qv_map(&[("v", QueryValue::Bin(vec![1, 2]))]),
        qv_map(&[("v", QueryValue::Bin(vec![1, 2]))]),
        qv_map(&[("v", QueryValue::Int(99))]),
    ];

    let result = apply_distinct_qv(qvs);

    // Two identical Bin → collapse to one; Int(99) is distinct
    assert_eq!(result.len(), 2);
    assert_eq!(result[1], qv_map(&[("v", QueryValue::Int(99))]));
}

#[test]
fn distinct_qv_null_and_nested_map() {
    let nested = qv_map(&[("x", QueryValue::Int(1)), ("y", QueryValue::Null)]);
    let qvs = vec![
        qv_map(&[("a", QueryValue::Null)]),
        qv_map(&[("a", nested.clone())]),
        qv_map(&[("a", QueryValue::Null)]),
        qv_map(&[("a", nested.clone())]),
    ];

    let result = apply_distinct_qv(qvs);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0], qv_map(&[("a", QueryValue::Null)]));
    assert_eq!(result[1], qv_map(&[("a", nested)]));
}

// ============================================================================
// Order By (Stage C)
// ============================================================================

#[test]
fn order_by_qv_int_asc() {
    let mut qvs = vec![
        qv_map(&[("age", QueryValue::Int(35))]),
        qv_map(&[("age", QueryValue::Int(25))]),
        qv_map(&[("age", QueryValue::Int(30))]),
    ];

    let order = OrderBy::asc("age");
    apply_order_by_qv(&mut qvs, &order);

    assert_eq!(qvs[0]["age"], QueryValue::Int(25));
    assert_eq!(qvs[1]["age"], QueryValue::Int(30));
    assert_eq!(qvs[2]["age"], QueryValue::Int(35));
}

#[test]
fn order_by_qv_int_desc() {
    let mut qvs = vec![
        qv_map(&[("age", QueryValue::Int(25))]),
        qv_map(&[("age", QueryValue::Int(35))]),
        qv_map(&[("age", QueryValue::Int(30))]),
    ];

    let order = OrderBy::desc("age");
    apply_order_by_qv(&mut qvs, &order);

    assert_eq!(qvs[0]["age"], QueryValue::Int(35));
    assert_eq!(qvs[1]["age"], QueryValue::Int(30));
    assert_eq!(qvs[2]["age"], QueryValue::Int(25));
}

#[test]
fn order_by_qv_f64() {
    let mut qvs = vec![
        qv_map(&[("v", QueryValue::F64(3.5))]),
        qv_map(&[("v", QueryValue::F64(1.0))]),
        qv_map(&[("v", QueryValue::F64(2.25))]),
    ];

    let order = OrderBy::asc("v");
    apply_order_by_qv(&mut qvs, &order);

    assert_eq!(qvs[0]["v"], QueryValue::F64(1.0));
    assert_eq!(qvs[1]["v"], QueryValue::F64(2.25));
    assert_eq!(qvs[2]["v"], QueryValue::F64(3.5));
}

#[test]
fn order_by_qv_mixed_int_float() {
    let mut qvs = vec![
        qv_map(&[("v", QueryValue::F64(2.5))]),
        qv_map(&[("v", QueryValue::Int(1))]),
        qv_map(&[("v", QueryValue::Int(3))]),
        qv_map(&[("v", QueryValue::F64(0.5))]),
    ];

    let order = OrderBy::asc("v");
    apply_order_by_qv(&mut qvs, &order);

    assert_eq!(qvs[0]["v"], QueryValue::F64(0.5));
    assert_eq!(qvs[1]["v"], QueryValue::Int(1));
    assert_eq!(qvs[2]["v"], QueryValue::F64(2.5));
    assert_eq!(qvs[3]["v"], QueryValue::Int(3));
}

#[test]
fn order_by_qv_string() {
    let mut qvs = vec![
        qv_map(&[("s", QueryValue::Str("cherry".into()))]),
        qv_map(&[("s", QueryValue::Str("apple".into()))]),
        qv_map(&[("s", QueryValue::Str("banana".into()))]),
    ];

    let order = OrderBy::asc("s");
    apply_order_by_qv(&mut qvs, &order);

    assert_eq!(qvs[0]["s"], QueryValue::Str("apple".into()));
    assert_eq!(qvs[1]["s"], QueryValue::Str("banana".into()));
    assert_eq!(qvs[2]["s"], QueryValue::Str("cherry".into()));
}

#[test]
fn order_by_qv_null_first_last() {
    for nulls in [NullsOrder::First, NullsOrder::Last] {
        let mut qvs = vec![
            qv_map(&[("v", QueryValue::Int(10))]),
            qv_map(&[("v", QueryValue::Null)]),
            qv_map(&[("v", QueryValue::Int(5))]),
        ];

        let order = OrderBy::new([OrderByItem {
            field: vec!["v".into()],
            direction: OrderDirection::Asc,
            nulls: Some(nulls),
        }]);
        apply_order_by_qv(&mut qvs, &order);

        match nulls {
            NullsOrder::First => {
                assert!(qvs[0]["v"].is_null(), "null should be first");
                assert_eq!(qvs[1]["v"], QueryValue::Int(5));
                assert_eq!(qvs[2]["v"], QueryValue::Int(10));
            }
            NullsOrder::Last => {
                assert_eq!(qvs[0]["v"], QueryValue::Int(5));
                assert_eq!(qvs[1]["v"], QueryValue::Int(10));
                assert!(qvs[2]["v"].is_null(), "null should be last");
            }
        }
    }
}

#[test]
fn order_by_qv_desc_nulls_first_last() {
    for nulls in [NullsOrder::First, NullsOrder::Last] {
        let mut qvs = vec![
            qv_map(&[("v", QueryValue::Int(10))]),
            qv_map(&[("v", QueryValue::Null)]),
            qv_map(&[("v", QueryValue::Int(5))]),
        ];

        let order = OrderBy::new([OrderByItem {
            field: vec!["v".into()],
            direction: OrderDirection::Desc,
            nulls: Some(nulls),
        }]);
        apply_order_by_qv(&mut qvs, &order);

        match nulls {
            NullsOrder::First => {
                assert!(qvs[0]["v"].is_null(), "null should be first");
                assert_eq!(qvs[1]["v"], QueryValue::Int(10));
                assert_eq!(qvs[2]["v"], QueryValue::Int(5));
            }
            NullsOrder::Last => {
                assert_eq!(qvs[0]["v"], QueryValue::Int(10));
                assert_eq!(qvs[1]["v"], QueryValue::Int(5));
                assert!(qvs[2]["v"].is_null(), "null should be last");
            }
        }
    }
}

#[test]
fn order_by_qv_dec_numeric() {
    // Dec values sort numerically (Dec sort-key variant, exact Decimal: Ord).
    // [9.0, 10.0, 2.0] → numeric ascending: 2.0, 9.0, 10.0.
    let mut qvs = vec![
        qv_map(&[("d", QueryValue::Dec("9.0".parse().unwrap()))]),
        qv_map(&[("d", QueryValue::Dec("10.0".parse().unwrap()))]),
        qv_map(&[("d", QueryValue::Dec("2.0".parse().unwrap()))]),
    ];

    let order = OrderBy::asc("d");
    apply_order_by_qv(&mut qvs, &order);

    // Numeric order: 2.0 < 9.0 < 10.0
    let extract_dec_str = |qv: &QueryValue| match qv {
        QueryValue::Dec(d) => d.to_string(),
        _ => panic!("expected Dec"),
    };
    assert_eq!(extract_dec_str(&qvs[0]["d"]), "2.0");
    assert_eq!(extract_dec_str(&qvs[1]["d"]), "9.0");
    assert_eq!(extract_dec_str(&qvs[2]["d"]), "10.0");
}

#[test]
fn order_by_qv_bin_is_unsortable() {
    // Bin maps to Array in canonical key → SortKey::Other (unsortable).
    // Both values keep insertion order (stable sort).
    let mut qvs = vec![
        qv_map(&[
            ("b", QueryValue::Bin(vec![3, 2, 1])),
            ("n", QueryValue::Int(2)),
        ]),
        qv_map(&[
            ("b", QueryValue::Bin(vec![1, 2, 3])),
            ("n", QueryValue::Int(1)),
        ]),
    ];

    let order = OrderBy::asc("b");
    apply_order_by_qv(&mut qvs, &order);

    // Insertion order preserved (both are "Other")
    assert_eq!(qvs[0]["n"], QueryValue::Int(2));
    assert_eq!(qvs[1]["n"], QueryValue::Int(1));
}

#[test]
fn order_by_qv_multiple_fields() {
    let mut qvs = vec![
        qv_map(&[
            ("city", QueryValue::Str("NYC".into())),
            ("age", QueryValue::Int(35)),
        ]),
        qv_map(&[
            ("city", QueryValue::Str("LA".into())),
            ("age", QueryValue::Int(30)),
        ]),
        qv_map(&[
            ("city", QueryValue::Str("LA".into())),
            ("age", QueryValue::Int(25)),
        ]),
        qv_map(&[
            ("city", QueryValue::Str("NYC".into())),
            ("age", QueryValue::Int(30)),
        ]),
    ];

    let order = OrderBy::new([OrderByItem::asc("city"), OrderByItem::asc("age")]);
    apply_order_by_qv(&mut qvs, &order);

    assert_eq!(qvs[0]["city"], QueryValue::Str("LA".into()));
    assert_eq!(qvs[0]["age"], QueryValue::Int(25));
    assert_eq!(qvs[1]["city"], QueryValue::Str("LA".into()));
    assert_eq!(qvs[1]["age"], QueryValue::Int(30));
    assert_eq!(qvs[2]["city"], QueryValue::Str("NYC".into()));
    assert_eq!(qvs[2]["age"], QueryValue::Int(30));
    assert_eq!(qvs[3]["city"], QueryValue::Str("NYC".into()));
    assert_eq!(qvs[3]["age"], QueryValue::Int(35));
}

#[test]
fn order_by_qv_empty_and_single() {
    // Empty
    let mut qvs: Vec<QueryValue> = vec![];
    let order = OrderBy::asc("x");
    apply_order_by_qv(&mut qvs, &order);
    assert!(qvs.is_empty());

    // Single
    let mut qvs = vec![qv_map(&[("x", QueryValue::Int(1))])];
    apply_order_by_qv(&mut qvs, &order);
    assert_eq!(qvs.len(), 1);
    assert_eq!(qvs[0], qv_map(&[("x", QueryValue::Int(1))]));
}

// ============================================================================
// Combined: DISTINCT + ORDER BY + PAGINATION (Path A integration)
// ============================================================================

#[test]
fn combined_distinct_order_paginate_qv() {
    let qvs = vec![
        qv_map(&[("v", QueryValue::Int(3))]),
        qv_map(&[("v", QueryValue::Int(1))]),
        qv_map(&[("v", QueryValue::Int(3))]),
        qv_map(&[("v", QueryValue::Int(2))]),
        qv_map(&[("v", QueryValue::Int(1))]),
        qv_map(&[("v", QueryValue::Int(4))]),
    ];

    // qv path: distinct → sort asc → page [offset=1, limit=2]
    let q_distinct = apply_distinct_qv(qvs);
    // distinct gives [1, 2, 3, 4] (insertion-order dedup)
    assert_eq!(q_distinct.len(), 4);

    let mut q_sorted = q_distinct;
    apply_order_by_qv(&mut q_sorted, &OrderBy::asc("v"));
    // sorted: [1, 2, 3, 4]

    let (q_paged, q_info) = apply_pagination(
        q_sorted,
        &Pagination::LimitOffset {
            limit: Some(2),
            offset: 1,
        },
        true,
    );

    // page = [2, 3]
    assert_eq!(q_paged.len(), 2);
    assert_eq!(q_paged[0]["v"], QueryValue::Int(2));
    assert_eq!(q_paged[1]["v"], QueryValue::Int(3));
    let info = q_info.unwrap();
    assert_eq!(info.total_count, Some(4));
}

#[test]
fn combined_with_dec_divergence_case() {
    // Dec("1.0") appearing twice should dedup to one row (canonical-key),
    // then sort numerically among other Dec values.
    let qvs = vec![
        qv_map(&[("v", QueryValue::Dec("3.0".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Dec("1.0".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Dec("1.0".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Dec("2.0".parse().unwrap()))]),
    ];

    let q_distinct = apply_distinct_qv(qvs);
    // Dec("3.0"), Dec("1.0") [kept, second Dec("1.0") deduped], Dec("2.0")
    assert_eq!(q_distinct.len(), 3);

    let mut q_sorted = q_distinct;
    apply_order_by_qv(&mut q_sorted, &OrderBy::asc("v"));

    // Numeric order: 1.0 < 2.0 < 3.0
    let str_of = |qv: &QueryValue| match qv {
        QueryValue::Str(s) => s.clone(),
        QueryValue::Dec(d) => d.to_string(),
        _ => panic!("expected Str or Dec"),
    };
    assert_eq!(str_of(&q_sorted[0]["v"]), "1.0");
    assert_eq!(str_of(&q_sorted[1]["v"]), "2.0");
    assert_eq!(str_of(&q_sorted[2]["v"]), "3.0");
}

#[test]
fn order_by_qv_bool_asc_desc() {
    // ASC: false < true
    let mut qvs = vec![
        qv_map(&[("b", QueryValue::Bool(true))]),
        qv_map(&[("b", QueryValue::Bool(false))]),
        qv_map(&[("b", QueryValue::Bool(true))]),
        qv_map(&[("b", QueryValue::Bool(false))]),
    ];

    let order = OrderBy::asc("b");
    apply_order_by_qv(&mut qvs, &order);
    assert_eq!(qvs[0]["b"], QueryValue::Bool(false));
    assert_eq!(qvs[1]["b"], QueryValue::Bool(false));
    assert_eq!(qvs[2]["b"], QueryValue::Bool(true));
    assert_eq!(qvs[3]["b"], QueryValue::Bool(true));

    // DESC: true > false
    let mut qvs = vec![
        qv_map(&[("b", QueryValue::Bool(true))]),
        qv_map(&[("b", QueryValue::Bool(false))]),
    ];

    let order = OrderBy::desc("b");
    apply_order_by_qv(&mut qvs, &order);
    assert_eq!(qvs[0]["b"], QueryValue::Bool(true));
    assert_eq!(qvs[1]["b"], QueryValue::Bool(false));
}
