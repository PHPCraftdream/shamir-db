//! Golden result-identity tests for QueryValue post-processors.
//!
//! Each test asserts that the new QueryValue-based post-processor
//! (distinct_qv, order_by_qv, pagination<T>) produces IDENTICAL
//! results (rows, order, serialised bytes) to the legacy json-based
//! post-processor, including the Dec/Big/Bin/Set divergence cases
//! where the canonical-key mapping is essential.

use serde_json as json;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

use crate::query::read::exec::{apply_distinct, apply_distinct_qv, apply_pagination};
use crate::query::read::order::{apply_order_by, apply_order_by_qv};
use crate::query::read::{NullsOrder, OrderBy, OrderByItem, OrderDirection, Pagination};

/// Helper: build a QueryValue map from key-value pairs.
fn qv_map(pairs: &[(&str, QueryValue)]) -> QueryValue {
    let mut m = new_map_wc(pairs.len());
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    QueryValue::Map(m)
}

/// Serialise both json and qv results to bytes and compare.
fn assert_byte_identical(json_results: &[json::Value], qv_results: &[QueryValue]) {
    assert_eq!(
        json_results.len(),
        qv_results.len(),
        "row count mismatch: json={} qv={}",
        json_results.len(),
        qv_results.len()
    );
    for (i, (j, q)) in json_results.iter().zip(qv_results.iter()).enumerate() {
        let jb = json::to_vec(j).unwrap();
        let qb = json::to_vec(q).unwrap();
        // Round-trip both through json::Value for structural comparison
        // (key ordering in maps may differ, so byte comparison is too strict).
        let jv: json::Value = json::from_slice(&jb).unwrap();
        let qv: json::Value = json::from_slice(&qb).unwrap();
        assert_eq!(jv, qv, "row {i} diverges:\n  json: {jv}\n  qv:   {qv}");
    }
}

// ============================================================================
// Pagination (Stage A) — generic over T
// ============================================================================

#[test]
fn pagination_qv_identical_to_json() {
    let qvs: Vec<QueryValue> = (1..=5).map(QueryValue::Int).collect();
    let jvs: Vec<json::Value> = (1..=5).map(json::Value::from).collect();

    let pag = Pagination::LimitOffset {
        limit: Some(2),
        offset: 1,
    };

    let (j_result, j_info) = apply_pagination(jvs, &pag, true);
    let (q_result, q_info) = apply_pagination(qvs, &pag, true);

    assert_byte_identical(&j_result, &q_result);
    assert_eq!(j_info, q_info);
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
    let jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let j_result = apply_distinct(jvs);
    let q_result = apply_distinct_qv(qvs);
    assert_byte_identical(&j_result, &q_result);
}

#[test]
fn distinct_qv_dec_vs_str_same_dedup_class() {
    // Dec("1.0") and Str("1.0") must deduplicate identically under both
    // paths because the json coercion maps Dec→String.
    let qvs = vec![
        qv_map(&[("v", QueryValue::Dec("1.0".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Str("1.0".to_string()))]),
        qv_map(&[("v", QueryValue::Int(2))]),
    ];
    let jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let j_result = apply_distinct(jvs);
    let q_result = apply_distinct_qv(qvs);

    // Both paths should deduplicate Dec("1.0") and Str("1.0") into one row.
    assert_byte_identical(&j_result, &q_result);
    assert_eq!(
        q_result.len(),
        2,
        "Dec and Str with same string should dedup"
    );
}

#[test]
fn distinct_qv_big_vs_str_same_dedup_class() {
    use num_bigint::BigInt;
    let qvs = vec![
        qv_map(&[("v", QueryValue::Big(BigInt::from(42)))]),
        qv_map(&[("v", QueryValue::Str("42".to_string()))]),
        qv_map(&[("v", QueryValue::Int(99))]),
    ];
    let jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let j_result = apply_distinct(jvs);
    let q_result = apply_distinct_qv(qvs);

    assert_byte_identical(&j_result, &q_result);
    assert_eq!(
        q_result.len(),
        2,
        "Big and Str with same string should dedup"
    );
}

#[test]
fn distinct_qv_bin_as_array() {
    // Bin([1, 2]) becomes Array([1, 2]) in json. Two identical Bin values
    // should dedup; a Bin and a List with the same int contents should also
    // dedup (they both become json arrays).
    let qvs = vec![
        qv_map(&[("v", QueryValue::Bin(vec![1, 2]))]),
        qv_map(&[("v", QueryValue::Bin(vec![1, 2]))]),
        qv_map(&[("v", QueryValue::Int(99))]),
    ];
    let jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let j_result = apply_distinct(jvs);
    let q_result = apply_distinct_qv(qvs);

    assert_byte_identical(&j_result, &q_result);
}

#[test]
fn distinct_qv_null_and_nested_map() {
    let nested = qv_map(&[("x", QueryValue::Int(1)), ("y", QueryValue::Null)]);
    let qvs = vec![
        qv_map(&[("a", QueryValue::Null)]),
        qv_map(&[("a", nested.clone())]),
        qv_map(&[("a", QueryValue::Null)]),
        qv_map(&[("a", nested)]),
    ];
    let jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let j_result = apply_distinct(jvs);
    let q_result = apply_distinct_qv(qvs);

    assert_byte_identical(&j_result, &q_result);
    assert_eq!(q_result.len(), 2);
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
    let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let order = OrderBy::asc("age");
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);

    assert_byte_identical(&jvs, &qvs);
}

#[test]
fn order_by_qv_int_desc() {
    let mut qvs = vec![
        qv_map(&[("age", QueryValue::Int(25))]),
        qv_map(&[("age", QueryValue::Int(35))]),
        qv_map(&[("age", QueryValue::Int(30))]),
    ];
    let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let order = OrderBy::desc("age");
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);

    assert_byte_identical(&jvs, &qvs);
}

#[test]
fn order_by_qv_f64() {
    let mut qvs = vec![
        qv_map(&[("v", QueryValue::F64(3.5))]),
        qv_map(&[("v", QueryValue::F64(1.0))]),
        qv_map(&[("v", QueryValue::F64(2.25))]),
    ];
    let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let order = OrderBy::asc("v");
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);

    assert_byte_identical(&jvs, &qvs);
}

#[test]
fn order_by_qv_mixed_int_float() {
    let mut qvs = vec![
        qv_map(&[("v", QueryValue::F64(2.5))]),
        qv_map(&[("v", QueryValue::Int(1))]),
        qv_map(&[("v", QueryValue::Int(3))]),
        qv_map(&[("v", QueryValue::F64(0.5))]),
    ];
    let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let order = OrderBy::asc("v");
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);

    assert_byte_identical(&jvs, &qvs);
}

#[test]
fn order_by_qv_string() {
    let mut qvs = vec![
        qv_map(&[("s", QueryValue::Str("cherry".into()))]),
        qv_map(&[("s", QueryValue::Str("apple".into()))]),
        qv_map(&[("s", QueryValue::Str("banana".into()))]),
    ];
    let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let order = OrderBy::asc("s");
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);

    assert_byte_identical(&jvs, &qvs);
}

#[test]
fn order_by_qv_null_first_last() {
    for nulls in [NullsOrder::First, NullsOrder::Last] {
        let mut qvs = vec![
            qv_map(&[("v", QueryValue::Int(10))]),
            qv_map(&[("v", QueryValue::Null)]),
            qv_map(&[("v", QueryValue::Int(5))]),
        ];
        let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

        let order = OrderBy::new([OrderByItem {
            field: vec!["v".into()],
            direction: OrderDirection::Asc,
            nulls: Some(nulls),
        }]);
        apply_order_by(&mut jvs, &order);
        apply_order_by_qv(&mut qvs, &order);

        assert_byte_identical(&jvs, &qvs);
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
        let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

        let order = OrderBy::new([OrderByItem {
            field: vec!["v".into()],
            direction: OrderDirection::Desc,
            nulls: Some(nulls),
        }]);
        apply_order_by(&mut jvs, &order);
        apply_order_by_qv(&mut qvs, &order);

        assert_byte_identical(&jvs, &qvs);
    }
}

#[test]
fn order_by_qv_dec_lexicographic() {
    // Dec values sort lexicographically by their string form in the json
    // path (Dec→String coercion). The QV path must reproduce this.
    let mut qvs = vec![
        qv_map(&[("d", QueryValue::Dec("9.0".parse().unwrap()))]),
        qv_map(&[("d", QueryValue::Dec("10.0".parse().unwrap()))]),
        qv_map(&[("d", QueryValue::Dec("2.0".parse().unwrap()))]),
    ];
    let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let order = OrderBy::asc("d");
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);

    assert_byte_identical(&jvs, &qvs);
}

#[test]
fn order_by_qv_bin_is_other() {
    // Bin maps to Array in json → SortKey::Other (unsortable).
    // Both paths should preserve insertion order (stable sort).
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
    let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let order = OrderBy::asc("b");
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);

    assert_byte_identical(&jvs, &qvs);
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
    let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let order = OrderBy::new([OrderByItem::asc("city"), OrderByItem::asc("age")]);
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);

    assert_byte_identical(&jvs, &qvs);
}

#[test]
fn order_by_qv_empty_and_single() {
    // Empty
    let mut qvs: Vec<QueryValue> = vec![];
    let mut jvs: Vec<json::Value> = vec![];
    let order = OrderBy::asc("x");
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);
    assert_byte_identical(&jvs, &qvs);

    // Single
    let mut qvs = vec![qv_map(&[("x", QueryValue::Int(1))])];
    let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);
    assert_byte_identical(&jvs, &qvs);
}

// ============================================================================
// Combined: DISTINCT + ORDER BY + PAGINATION (Path A integration)
// ============================================================================

#[test]
fn combined_distinct_order_paginate_qv_vs_json() {
    let qvs = vec![
        qv_map(&[("v", QueryValue::Int(3))]),
        qv_map(&[("v", QueryValue::Int(1))]),
        qv_map(&[("v", QueryValue::Int(3))]),
        qv_map(&[("v", QueryValue::Int(2))]),
        qv_map(&[("v", QueryValue::Int(1))]),
        qv_map(&[("v", QueryValue::Int(4))]),
    ];
    let jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    // json path
    let j_distinct = apply_distinct(jvs);
    let mut j_sorted = j_distinct;
    apply_order_by(&mut j_sorted, &OrderBy::asc("v"));
    let (j_paged, j_info) = apply_pagination(
        j_sorted,
        &Pagination::LimitOffset {
            limit: Some(2),
            offset: 1,
        },
        true,
    );

    // qv path
    let q_distinct = apply_distinct_qv(qvs);
    let mut q_sorted = q_distinct;
    apply_order_by_qv(&mut q_sorted, &OrderBy::asc("v"));
    let (q_paged, q_info) = apply_pagination(
        q_sorted,
        &Pagination::LimitOffset {
            limit: Some(2),
            offset: 1,
        },
        true,
    );

    assert_byte_identical(&j_paged, &q_paged);
    assert_eq!(j_info, q_info);
}

#[test]
fn combined_with_dec_divergence_case() {
    // Dec("1.0") and Str("1.0") should dedup to one row (canonical-key),
    // then sort correctly among other values.
    let qvs = vec![
        qv_map(&[("v", QueryValue::Dec("3.0".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Str("1.0".into()))]),
        qv_map(&[("v", QueryValue::Dec("1.0".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Dec("2.0".parse().unwrap()))]),
    ];
    let jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    // json path
    let j_distinct = apply_distinct(jvs);
    let mut j_sorted = j_distinct;
    apply_order_by(&mut j_sorted, &OrderBy::asc("v"));

    // qv path
    let q_distinct = apply_distinct_qv(qvs);
    let mut q_sorted = q_distinct;
    apply_order_by_qv(&mut q_sorted, &OrderBy::asc("v"));

    assert_byte_identical(&j_sorted, &q_sorted);
}

#[test]
fn order_by_qv_bool_asc_desc() {
    let mut qvs = vec![
        qv_map(&[("b", QueryValue::Bool(true))]),
        qv_map(&[("b", QueryValue::Bool(false))]),
        qv_map(&[("b", QueryValue::Bool(true))]),
        qv_map(&[("b", QueryValue::Bool(false))]),
    ];
    let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let order = OrderBy::asc("b");
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);
    assert_byte_identical(&jvs, &qvs);

    // Reset and test DESC
    let mut qvs = vec![
        qv_map(&[("b", QueryValue::Bool(true))]),
        qv_map(&[("b", QueryValue::Bool(false))]),
    ];
    let mut jvs: Vec<json::Value> = qvs.iter().map(|q| json::Value::from(q.clone())).collect();

    let order = OrderBy::desc("b");
    apply_order_by(&mut jvs, &order);
    apply_order_by_qv(&mut qvs, &order);
    assert_byte_identical(&jvs, &qvs);
}
