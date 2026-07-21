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

use bytes::Bytes;
use rust_decimal::Decimal;
use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::exec::{apply_distinct_qv, apply_pagination, apply_select_value};
use crate::query::read::order::{apply_order_by_qv, apply_order_by_topk};
use crate::query::read::{
    aggregate::AggAccum, apply_aggregate_all, apply_group_by, AggFunc, AggregateField, GroupBy,
    NullsOrder, OrderBy, OrderByItem, OrderDirection, Pagination, Select,
};
use shamir_query_builder::select;
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

/// Helper: build a QueryValue map from key-value pairs.
fn qv_map(pairs: &[(&str, QueryValue)]) -> QueryValue {
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
fn distinct_qv_dec_vs_str_same_dedup_class() {
    // Dec("1.0") and Str("1.0") must deduplicate identically because
    // the canonical-key mapping converts Dec→String ("1.0").
    let qvs = vec![
        qv_map(&[("v", QueryValue::Dec("1.0".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Str("1.0".to_string()))]),
        qv_map(&[("v", QueryValue::Int(2))]),
    ];

    let result = apply_distinct_qv(qvs);

    // Dec("1.0") and Str("1.0") share the same canonical key → deduplicate
    assert_eq!(result.len(), 2, "Dec and Str with same string should dedup");
    // First seen (Dec) is kept; Int(2) is distinct
    assert!(
        matches!(&result[0]["v"], QueryValue::Dec(_)),
        "first row should have Dec value"
    );
    assert_eq!(result[1]["v"], QueryValue::Int(2));
}

#[test]
fn distinct_qv_big_vs_str_same_dedup_class() {
    use num_bigint::BigInt;
    let qvs = vec![
        qv_map(&[("v", QueryValue::Big(BigInt::from(42)))]),
        qv_map(&[("v", QueryValue::Str("42".to_string()))]),
        qv_map(&[("v", QueryValue::Int(99))]),
    ];

    let result = apply_distinct_qv(qvs);

    // Big(42) and Str("42") share the same canonical key → deduplicate
    assert_eq!(result.len(), 2, "Big and Str with same string should dedup");
    assert!(
        matches!(&result[0]["v"], QueryValue::Big(_)),
        "first row should have Big value"
    );
    assert_eq!(result[1]["v"], QueryValue::Int(99));
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

// ============================================================================
// Stage D: apply_select_value explicit assertions
// ============================================================================

/// Intern a string, returning its u64 id.
fn intern(interner: &Interner, s: &str) -> u64 {
    match interner.touch_ind(s) {
        Ok(TouchInd::New(k)) | Ok(TouchInd::Exists(k)) => k.id(),
        Err(e) => panic!("intern failed: {e}"),
    }
}

/// Build a record: `{ name: Str, age: Int, city: Str, score: F64 }`.
fn make_record(interner: &Interner, name: &str, age: i64, city: &str, score: f64) -> InnerValue {
    let mut map = new_map();
    map.insert(
        InternerKey::new(intern(interner, "name")),
        InnerValue::Str(name.into()),
    );
    map.insert(
        InternerKey::new(intern(interner, "age")),
        InnerValue::Int(age),
    );
    map.insert(
        InternerKey::new(intern(interner, "city")),
        InnerValue::Str(city.into()),
    );
    map.insert(
        InternerKey::new(intern(interner, "score")),
        InnerValue::F64(score),
    );
    InnerValue::Map(map)
}

/// S4 helper: encode `InnerValue` records to `Bytes` for the lens-fed
/// aggregate pipeline (`apply_group_by` / `apply_aggregate_all`).
fn to_bytes_records(records: &[(RecordId, InnerValue)]) -> Vec<(RecordId, Bytes)> {
    records
        .iter()
        .map(|(id, iv)| {
            let bytes = iv.to_bytes().expect("encode InnerValue to bytes");
            (*id, bytes)
        })
        .collect()
}

fn make_test_records(interner: &Interner) -> Vec<(RecordId, InnerValue)> {
    vec![
        (
            RecordId::new(),
            make_record(interner, "Alice", 30, "NYC", 1.5),
        ),
        (RecordId::new(), make_record(interner, "Bob", 25, "LA", 2.5)),
        (
            RecordId::new(),
            make_record(interner, "Carol", 35, "NYC", 3.5),
        ),
        (
            RecordId::new(),
            make_record(interner, "Dave", 25, "LA", 0.5),
        ),
    ]
}

#[test]
fn select_value_specific_fields() {
    let interner = Interner::default();
    let records = make_test_records(&interner);
    let select = Select::fields(["name", "age", "city"]);

    let result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(result.len(), 4);

    assert_eq!(result[0]["name"], QueryValue::Str("Alice".into()));
    assert_eq!(result[0]["age"], QueryValue::Int(30));
    assert_eq!(result[0]["city"], QueryValue::Str("NYC".into()));
    assert_eq!(result[1]["name"], QueryValue::Str("Bob".into()));
    assert_eq!(result[1]["age"], QueryValue::Int(25));
    assert_eq!(result[2]["name"], QueryValue::Str("Carol".into()));
    assert_eq!(result[2]["age"], QueryValue::Int(35));
}

#[test]
fn select_value_all_returns_all_fields() {
    let interner = Interner::default();
    let records = make_test_records(&interner);
    let select = Select::all();

    let result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(result.len(), 4);

    // All four fields present
    assert_eq!(result[0]["name"], QueryValue::Str("Alice".into()));
    assert_eq!(result[0]["age"], QueryValue::Int(30));
    assert_eq!(result[0]["city"], QueryValue::Str("NYC".into()));
    assert_eq!(result[0]["score"], QueryValue::F64(1.5));
}

#[test]
fn path_b_distinct_order_qv() {
    // Path B: apply_select_value -> distinct_qv -> order_by_qv
    let interner = Interner::default();
    let records = make_test_records(&interner);
    let select = Select::fields(["city", "age"]);

    let qv_result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    // 4 rows: {city:NYC,age:30},{city:LA,age:25},{city:NYC,age:35},{city:LA,age:25}
    // After distinct: {NYC,30},{LA,25},{NYC,35} → 3 distinct rows
    let q_distinct = apply_distinct_qv(qv_result);
    assert_eq!(q_distinct.len(), 3);

    let mut q_sorted = q_distinct;
    apply_order_by_qv(&mut q_sorted, &OrderBy::asc("age"));

    // sorted by age: 25, 30, 35
    assert_eq!(q_sorted[0]["age"], QueryValue::Int(25));
    assert_eq!(q_sorted[1]["age"], QueryValue::Int(30));
    assert_eq!(q_sorted[2]["age"], QueryValue::Int(35));
}

// ── Stage E: aggregate pipeline QueryValue assertions ───────────────────────

#[test]
fn aggregate_group_by_all_funcs() {
    // GROUP BY city + SUM/AVG/MIN/MAX/COUNT on age, score.
    let interner = Interner::default();
    let records = make_test_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy::new(["city"]);
    let select = Select {
        items: vec![
            select::field("city"),
            select::count_all("cnt"),
            select::sum("age", "sum_age"),
            select::avg("age", "avg_age"),
            select::min("age", "min_age"),
            select::max("age", "max_age"),
            select::sum("score", "sum_score"),
            select::avg("score", "avg_score"),
        ],
        distinct: false,
    };

    let result = apply_group_by(
        &to_bytes_records(&records),
        &group_by,
        &select,
        &interner,
        &ctx,
    );

    // Groups sorted alphabetically: LA, NYC.
    assert_eq!(result.len(), 2);
    assert_eq!(result[0]["city"], QueryValue::Str("LA".into()));
    assert_eq!(result[0]["cnt"], QueryValue::Int(2));
    assert_eq!(result[0]["sum_age"], QueryValue::Int(50));
    assert_eq!(result[0]["avg_age"], QueryValue::F64(25.0));
    assert_eq!(result[0]["min_age"], QueryValue::Int(25));
    assert_eq!(result[0]["max_age"], QueryValue::Int(25));
    assert_eq!(result[1]["city"], QueryValue::Str("NYC".into()));
    assert_eq!(result[1]["cnt"], QueryValue::Int(2));
    assert_eq!(result[1]["sum_age"], QueryValue::Int(65));
    assert_eq!(result[1]["avg_age"], QueryValue::F64(32.5));
    assert_eq!(result[1]["min_age"], QueryValue::Int(30));
    assert_eq!(result[1]["max_age"], QueryValue::Int(35));
}

#[test]
fn aggregate_sum_float_serialisation() {
    // Sum of floats must produce F64 that serialises identically via msgpack.
    // Total score = 1.5+2.5+3.5+0.5 = 8.0.
    let interner = Interner::default();
    let records = make_test_records(&interner);

    let select = Select {
        items: vec![select::sum("score", "total_score")],
        distinct: false,
    };

    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(result.len(), 1);

    let qv_bytes = rmp_serde::to_vec_named(&result[0]).unwrap();

    let total = 1.5 + 2.5 + 3.5 + 0.5; // = 8.0
    let expected_map = QueryValue::Map({
        let mut m = new_map_wc(1);
        m.insert("total_score".to_string(), QueryValue::F64(total));
        m
    });
    let expected_bytes = rmp_serde::to_vec_named(&expected_map).unwrap();

    assert_eq!(
        qv_bytes, expected_bytes,
        "Sum(float) F64 msgpack serialisation must match expected"
    );
}

#[test]
fn aggregate_having_filters_correctly() {
    // GROUP BY city + HAVING sum_age > 55 should keep only NYC (sum=65).
    let interner = Interner::default();
    let records = make_test_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy {
        fields: vec![vec!["city".into()]],
        having: Some(shamir_query_builder::filter::gt("sum_age", 55)),
    };
    let select = Select {
        items: vec![select::field("city"), select::sum("age", "sum_age")],
        distinct: false,
    };

    let result = apply_group_by(
        &to_bytes_records(&records),
        &group_by,
        &select,
        &interner,
        &ctx,
    );

    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["city"], QueryValue::Str("NYC".into()));
    assert_eq!(result[0]["sum_age"], QueryValue::Int(65));
}

#[test]
fn aggregate_avg_float_serialisation() {
    // Avg produces F64 — must serialise identically via msgpack.
    // avg(score) = (1.5+2.5+3.5+0.5)/4 = 2.0
    let interner = Interner::default();
    let records = make_test_records(&interner);

    let select = Select {
        items: vec![select::avg("score", "avg_score")],
        distinct: false,
    };

    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(result.len(), 1);

    let qv_bytes = rmp_serde::to_vec_named(&result[0]).unwrap();

    let avg = (1.5 + 2.5 + 3.5 + 0.5) / 4.0; // = 2.0
    let expected_map = QueryValue::Map({
        let mut m = new_map_wc(1);
        m.insert("avg_score".to_string(), QueryValue::F64(avg));
        m
    });
    let expected_bytes = rmp_serde::to_vec_named(&expected_map).unwrap();

    assert_eq!(
        qv_bytes, expected_bytes,
        "Avg F64 msgpack serialisation must match expected"
    );
}

#[test]
fn aggregate_count_as_int() {
    // Count produces Int(i64) — must serialise identically via msgpack.
    let interner = Interner::default();
    let records = make_test_records(&interner);

    let select = Select {
        items: vec![select::count_all("cnt")],
        distinct: false,
    };

    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    let qv_bytes = rmp_serde::to_vec_named(&result[0]).unwrap();

    let expected_map = QueryValue::Map({
        let mut m = new_map_wc(1);
        m.insert("cnt".to_string(), QueryValue::Int(4));
        m
    });
    let expected_bytes = rmp_serde::to_vec_named(&expected_map).unwrap();

    assert_eq!(
        qv_bytes, expected_bytes,
        "Count msgpack serialisation must match expected"
    );
}

#[test]
fn aggregate_all_no_group() {
    // apply_aggregate_all: SUM + AVG + MIN + MAX + COUNT without GROUP BY.
    let interner = Interner::default();
    let records = make_test_records(&interner);

    let select = Select {
        items: vec![
            select::count_all("cnt"),
            select::sum("age", "sum_age"),
            select::avg("age", "avg_age"),
            select::min("age", "min_age"),
            select::max("age", "max_age"),
        ],
        distinct: false,
    };

    let result = apply_aggregate_all(
        &to_bytes_records(&records),
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );

    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["cnt"], QueryValue::Int(4));
    assert_eq!(result[0]["sum_age"], QueryValue::Int(115));
    // avg_age = 115/4 = 28.75
    assert_eq!(result[0]["avg_age"], QueryValue::F64(28.75));
    assert_eq!(result[0]["min_age"], QueryValue::Int(25));
    assert_eq!(result[0]["max_age"], QueryValue::Int(35));
}

// ── Top-K correctness fuzz ─────────────────────────────────────────────────

/// Verify that `apply_order_by_topk` produces byte-identical results to
/// `apply_order_by_qv` + skip/take for random data, both ASC and DESC.
#[test]
fn apply_order_by_topk_byte_identical() {
    let k = 10usize;
    let n = 1000usize;

    // Build 1000 random-ish records: {v: i64, name: String}
    let mut records: Vec<QueryValue> = Vec::with_capacity(n);
    for i in 0..n {
        let mut m = new_map_wc(2);
        // Deterministic pseudo-random value
        let v = ((i as i64).wrapping_mul(0x5DEECE66D) ^ (i as i64 * 37)) % 500;
        let name = format!("rec_{:04x}", i);
        m.insert("v".to_string(), QueryValue::Int(v));
        m.insert("name".to_string(), QueryValue::Str(name));
        records.push(QueryValue::Map(m));
    }

    // Test both ASC and DESC
    for direction in [OrderDirection::Asc, OrderDirection::Desc] {
        let order = OrderBy {
            items: vec![OrderByItem {
                field: vec!["v".to_string()],
                direction,
                nulls: None,
            }],
        };

        // Reference: full sort + skip/take
        let mut full_sorted = records.clone();
        apply_order_by_qv(&mut full_sorted, &order);
        let expected: Vec<QueryValue> = full_sorted.into_iter().take(k).collect();

        // Top-K path
        let topk_result = apply_order_by_topk(records.clone(), &order, 0, k);

        // Byte-identical comparison via msgpack serialisation
        let expected_bytes = rmp_serde::to_vec_named(&expected).unwrap();
        let topk_bytes = rmp_serde::to_vec_named(&topk_result).unwrap();

        assert_eq!(
            expected_bytes, topk_bytes,
            "topk result must be byte-identical to full-sort+truncate ({direction:?})"
        );
    }

    // Test with skip > 0
    let order = OrderBy::asc("v");
    let skip = 5usize;
    let take = 10usize;

    let mut full_sorted = records.clone();
    apply_order_by_qv(&mut full_sorted, &order);
    let expected: Vec<QueryValue> = full_sorted.into_iter().skip(skip).take(take).collect();

    let topk_result = apply_order_by_topk(records.clone(), &order, skip, take);

    let expected_bytes = rmp_serde::to_vec_named(&expected).unwrap();
    let topk_bytes = rmp_serde::to_vec_named(&topk_result).unwrap();

    assert_eq!(
        expected_bytes, topk_bytes,
        "topk with skip must be byte-identical to full-sort+skip+take"
    );
}

// ============================================================================
// Dec aggregate + ORDER BY regression coverage (the "Dec blind spot" fix)
// ============================================================================

/// Build a record: `{ name: Str, price: Dec }` — a genuine in-memory Dec
/// field. `scalar_at` returns `None` for this (no `ScalarRef::Dec` variant),
/// so Sum/Avg/Min/Max must use the `materialize_at` fallback.
///
/// NOTE: Dec does not survive the msgpack round-trip (it serialises as a
/// string and deserialises as `Str`), so these records are fed to `AggAccum`
/// directly as in-memory `InnerValue` rather than through `to_bytes_records`.
fn make_dec_record(interner: &Interner, name: &str, price: Decimal) -> InnerValue {
    let mut map = new_map();
    map.insert(
        InternerKey::new(intern(interner, "name")),
        InnerValue::Str(name.into()),
    );
    map.insert(
        InternerKey::new(intern(interner, "price")),
        InnerValue::Dec(price),
    );
    InnerValue::Map(map)
}

fn make_dec_test_records(interner: &Interner) -> Vec<InnerValue> {
    // Non-monotonic insertion order: 9.5, 10.5, 2.0, 2.0.
    vec![
        make_dec_record(interner, "A", "9.5".parse().unwrap()),
        make_dec_record(interner, "B", "10.5".parse().unwrap()),
        make_dec_record(interner, "C", "2.0".parse().unwrap()),
        make_dec_record(interner, "D", "2.0".parse().unwrap()),
    ]
}

/// `sum(price)` over a Dec column: 9.5 + 10.5 + 2.0 + 2.0 = 24.0. Before the
/// fix, Sum silently skipped every Dec row (`scalar_at` → None, no fallback)
/// and returned `Int(0)`.
#[test]
fn aggregate_sum_over_dec_column() {
    let interner = Interner::default();
    let records = make_dec_test_records(&interner);
    let field = AggregateField::Field(vec!["price".into()]);
    let mut acc = AggAccum::new(AggFunc::Sum, &field, &interner);
    for rec in &records {
        acc.step(rec);
    }
    // Dec values flow into the f64 lane via the materialize fallback.
    assert_eq!(acc.finish(&interner), QueryValue::F64(24.0));
}

/// `avg(price)` over a Dec column: 24.0 / 4 = 6.0. Before the fix, Avg
/// returned `Null` (count stayed 0).
#[test]
fn aggregate_avg_over_dec_column() {
    let interner = Interner::default();
    let records = make_dec_test_records(&interner);
    let field = AggregateField::Field(vec!["price".into()]);
    let mut acc = AggAccum::new(AggFunc::Avg, &field, &interner);
    for rec in &records {
        acc.step(rec);
    }
    assert_eq!(acc.finish(&interner), QueryValue::F64(6.0));
}

/// `min(price)` over a Dec column: inserted in order [9.5, 10.5, 2.0, 2.0] —
/// the true min is 2.0 (not whichever row was scanned first). This proves the
/// existing Min container fallback now correctly compares Dec-vs-Dec once
/// `compare_values` has the new Dec arms.
#[test]
fn aggregate_min_over_dec_column() {
    let interner = Interner::default();
    let records = make_dec_test_records(&interner);
    let field = AggregateField::Field(vec!["price".into()]);
    let mut acc = AggAccum::new(AggFunc::Min, &field, &interner);
    for rec in &records {
        acc.step(rec);
    }
    assert_eq!(
        acc.finish(&interner),
        QueryValue::Dec("2.0".parse().unwrap())
    );
}

/// `max(price)` over a Dec column: true max is 10.5 (not the first row 9.5).
#[test]
fn aggregate_max_over_dec_column() {
    let interner = Interner::default();
    let records = make_dec_test_records(&interner);
    let field = AggregateField::Field(vec!["price".into()]);
    let mut acc = AggAccum::new(AggFunc::Max, &field, &interner);
    for rec in &records {
        acc.step(rec);
    }
    assert_eq!(
        acc.finish(&interner),
        QueryValue::Dec("10.5".parse().unwrap())
    );
}

/// ORDER BY over a Dec column: [9.5, 10.5, 2.0] must sort numerically as
/// [2.0, 9.5, 10.5], NOT lexicographically as ["10.5", "2.0", "9.5"].
#[test]
fn order_by_dec_numeric_not_lexicographic() {
    let mut qvs = vec![
        qv_map(&[("v", QueryValue::Dec("9.5".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Dec("10.5".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Dec("2.0".parse().unwrap()))]),
    ];

    apply_order_by_qv(&mut qvs, &OrderBy::asc("v"));

    // Numeric ascending: 2.0, 9.5, 10.5.
    assert_eq!(qvs[0]["v"], QueryValue::Dec("2.0".parse().unwrap()));
    assert_eq!(qvs[1]["v"], QueryValue::Dec("9.5".parse().unwrap()));
    assert_eq!(qvs[2]["v"], QueryValue::Dec("10.5".parse().unwrap()));
}

/// ORDER BY DESC over Dec: [9.5, 10.5, 2.0] → [10.5, 9.5, 2.0].
#[test]
fn order_by_dec_desc() {
    let mut qvs = vec![
        qv_map(&[("v", QueryValue::Dec("9.5".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Dec("10.5".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Dec("2.0".parse().unwrap()))]),
    ];

    apply_order_by_qv(&mut qvs, &OrderBy::desc("v"));

    assert_eq!(qvs[0]["v"], QueryValue::Dec("10.5".parse().unwrap()));
    assert_eq!(qvs[1]["v"], QueryValue::Dec("9.5".parse().unwrap()));
    assert_eq!(qvs[2]["v"], QueryValue::Dec("2.0".parse().unwrap()));
}

/// Cross-type ORDER BY: a mix of Dec, Int, and F64 values in the same sort
/// key position — they must compare numerically against each other, not fall
/// to the `_ => Equal` arbitrary-order arm.
#[test]
fn order_by_cross_type_dec_int_f64() {
    let mut qvs = vec![
        qv_map(&[("v", QueryValue::Dec("9.5".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Int(2))]),
        qv_map(&[("v", QueryValue::F64(10.5))]),
        qv_map(&[("v", QueryValue::Dec("5.0".parse().unwrap()))]),
    ];

    apply_order_by_qv(&mut qvs, &OrderBy::asc("v"));

    // Numeric ascending: Int(2), Dec(5.0), Dec(9.5), F64(10.5).
    assert_eq!(qvs[0]["v"], QueryValue::Int(2));
    assert_eq!(qvs[1]["v"], QueryValue::Dec("5.0".parse().unwrap()));
    assert_eq!(qvs[2]["v"], QueryValue::Dec("9.5".parse().unwrap()));
    assert_eq!(qvs[3]["v"], QueryValue::F64(10.5));
}

/// FG-1 regression: `compare_values` (the engine-level comparison helper used
/// by `FilterNode::ValueCompare`, aggregate Min/Max, etc.) has explicit
/// `Big`↔`Int` arms that use the f64 fallback. This test confirms a
/// collection containing BOTH ordinary `Int` values and promoted `Big`
/// values compares correctly relative to each other via `compare_values`.
///
/// NOTE: the ORDER BY path (`apply_order_by_qv`) uses a separate
/// `QvSortKey` that maps `Big(b)` → `Str(b.to_string())` (lexicographic).
/// `QvSortKey` has no `I64`↔`Str` cross-type arm, so mixed Int+Big ORDER
/// BY falls to `_ => Equal` (preserving insertion order). This is a known
/// limitation of the sort-key representation, NOT of `compare_values`.
#[test]
fn order_by_mixed_int_and_big_compare_values_works() {
    use crate::query::filter::eval::compare_values;
    use num_bigint::BigInt;
    use std::cmp::Ordering;

    let int_small = QueryValue::Int(50);
    let int_large = QueryValue::Int(100);
    let big_overflow = QueryValue::Big(BigInt::from(i64::MAX as u64 + 1));
    let big_max = QueryValue::Big(BigInt::from(u64::MAX));

    // Int vs Int — exact.
    assert_eq!(compare_values(&int_small, &int_large), Some(Ordering::Less));

    // Int vs Big — f64 fallback. Int(50) << Big(i64::MAX+1 ≈ 9.2e18).
    assert_eq!(
        compare_values(&int_small, &big_overflow),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare_values(&int_large, &big_overflow),
        Some(Ordering::Less)
    );

    // Big vs Big — f64 fallback. i64::MAX+1 ≈ 9.2e18 < u64::MAX ≈ 1.8e19.
    assert_eq!(
        compare_values(&big_overflow, &big_max),
        Some(Ordering::Less)
    );

    // Symmetry.
    assert_eq!(
        compare_values(&big_overflow, &int_small),
        Some(Ordering::Greater)
    );
    assert_eq!(
        compare_values(&big_max, &big_overflow),
        Some(Ordering::Greater)
    );
}

// ============================================================================
// Sum integer-overflow regression (the "Sum accumulator unchecked i64
// overflow" fix): values that individually fit in i64 but whose running
// total crosses ±2^63 must NOT panic and must lift to the F64 lane.
// ============================================================================

/// Build a record: `{ v: Int }` — a genuine in-memory Int field. `scalar_at`
/// returns `ScalarRef::Int`, so Sum's integer accumulator is exercised.
fn make_int_record(interner: &Interner, v: i64) -> InnerValue {
    let mut map = new_map();
    map.insert(InternerKey::new(intern(interner, "v")), InnerValue::Int(v));
    InnerValue::Map(map)
}

/// `sum(v)` over two Int rows whose total exceeds `i64::MAX`: must NOT panic
/// and must return a `F64` close to the true mathematical sum, not a
/// wrapped/garbage `Int`.
#[test]
fn aggregate_sum_int_overflow_lifts_to_f64() {
    let interner = Interner::default();
    let half = i64::MAX / 2 + 1; // 4611686018427387904
    let records = vec![
        make_int_record(&interner, half),
        make_int_record(&interner, half),
    ];
    let field = AggregateField::Field(vec!["v".into()]);
    let mut acc = AggAccum::new(AggFunc::Sum, &field, &interner);
    for rec in &records {
        acc.step(rec);
    }
    // True sum = 2 * half = i64::MAX + 1 = 9223372036854775808.
    let result = acc.finish(&interner);
    match result {
        QueryValue::F64(f) => assert!((f - (i64::MAX as f64 + 1.0)).abs() < 1.0),
        other => panic!("expected F64 after overflow, got {other:?}"),
    }
}

/// `sum(v)` over all-Int values that stay well within `i64` range must still
/// return `Int`, not be lifted to float unnecessarily.
#[test]
fn aggregate_sum_int_no_overflow_stays_int() {
    let interner = Interner::default();
    let records = vec![
        make_int_record(&interner, 1),
        make_int_record(&interner, 2),
        make_int_record(&interner, 3),
    ];
    let field = AggregateField::Field(vec!["v".into()]);
    let mut acc = AggAccum::new(AggFunc::Sum, &field, &interner);
    for rec in &records {
        acc.step(rec);
    }
    assert_eq!(acc.finish(&interner), QueryValue::Int(6));
}
