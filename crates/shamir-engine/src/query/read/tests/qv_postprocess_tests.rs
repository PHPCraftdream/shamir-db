//! Self-contained tests for QueryValue post-processors.
//!
//! Each test asserts that the QueryValue-based post-processor
//! (distinct_qv, order_by_qv, pagination<T>) produces the correct
//! result against explicit expected values, including the Dec/Big/Bin/Set
//! edge cases where canonical-key mapping is essential for correct dedup.
//!
//! The old JSON-twin parity checks (comparing against the now-deleted
//! apply_distinct / apply_select JSON path) have been replaced with
//! concrete assertions against known-correct QueryValue results.

use bytes::Bytes;
use serde_json as json;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::exec::{apply_distinct_qv, apply_pagination, apply_select_value};
use crate::query::read::order::apply_order_by_qv;
use crate::query::read::{
    apply_aggregate_all, apply_group_by, GroupBy, NullsOrder, OrderBy, OrderByItem, OrderDirection,
    Pagination, Select,
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

/// Serialise a QueryValue slice to json::Values for comparison.
fn to_json(qvs: &[QueryValue]) -> Vec<json::Value> {
    qvs.iter().map(|v| json::to_value(v).unwrap()).collect()
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
    let r = to_json(&result);
    assert_eq!(r[0]["a"], 1);
    assert_eq!(r[1]["a"], 2);
    assert_eq!(r[2]["a"], 3);
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
    let r = to_json(&result);
    assert_eq!(r[0]["v"], "1.0"); // Dec serialises to String in JSON
    assert_eq!(r[1]["v"], 2);
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
    let r = to_json(&result);
    assert_eq!(r[0]["v"], "42"); // Big serialises to String in JSON
    assert_eq!(r[1]["v"], 99);
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

    let r = to_json(&qvs);
    assert_eq!(r[0]["age"], 25);
    assert_eq!(r[1]["age"], 30);
    assert_eq!(r[2]["age"], 35);
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

    let r = to_json(&qvs);
    assert_eq!(r[0]["age"], 35);
    assert_eq!(r[1]["age"], 30);
    assert_eq!(r[2]["age"], 25);
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

    let r = to_json(&qvs);
    assert_eq!(r[0]["v"], 1.0);
    assert_eq!(r[1]["v"], 2.25);
    assert_eq!(r[2]["v"], 3.5);
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

    let r = to_json(&qvs);
    assert_eq!(r[0]["v"], 0.5);
    assert_eq!(r[1]["v"], 1);
    assert_eq!(r[2]["v"], 2.5);
    assert_eq!(r[3]["v"], 3);
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

    let r = to_json(&qvs);
    assert_eq!(r[0]["s"], "apple");
    assert_eq!(r[1]["s"], "banana");
    assert_eq!(r[2]["s"], "cherry");
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

        let r = to_json(&qvs);
        match nulls {
            NullsOrder::First => {
                assert!(r[0]["v"].is_null(), "null should be first");
                assert_eq!(r[1]["v"], 5);
                assert_eq!(r[2]["v"], 10);
            }
            NullsOrder::Last => {
                assert_eq!(r[0]["v"], 5);
                assert_eq!(r[1]["v"], 10);
                assert!(r[2]["v"].is_null(), "null should be last");
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

        let r = to_json(&qvs);
        match nulls {
            NullsOrder::First => {
                assert!(r[0]["v"].is_null(), "null should be first");
                assert_eq!(r[1]["v"], 10);
                assert_eq!(r[2]["v"], 5);
            }
            NullsOrder::Last => {
                assert_eq!(r[0]["v"], 10);
                assert_eq!(r[1]["v"], 5);
                assert!(r[2]["v"].is_null(), "null should be last");
            }
        }
    }
}

#[test]
fn order_by_qv_dec_lexicographic() {
    // Dec values sort lexicographically by their string form
    // (Dec→String canonical key). "10.0" < "2.0" < "9.0" lexicographically.
    let mut qvs = vec![
        qv_map(&[("d", QueryValue::Dec("9.0".parse().unwrap()))]),
        qv_map(&[("d", QueryValue::Dec("10.0".parse().unwrap()))]),
        qv_map(&[("d", QueryValue::Dec("2.0".parse().unwrap()))]),
    ];

    let order = OrderBy::asc("d");
    apply_order_by_qv(&mut qvs, &order);

    let r = to_json(&qvs);
    // Lexicographic order: "10.0" < "2.0" < "9.0"
    assert_eq!(r[0]["d"], "10.0");
    assert_eq!(r[1]["d"], "2.0");
    assert_eq!(r[2]["d"], "9.0");
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
    let r = to_json(&qvs);
    assert_eq!(r[0]["n"], 2);
    assert_eq!(r[1]["n"], 1);
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

    let r = to_json(&qvs);
    assert_eq!(r[0]["city"], "LA");
    assert_eq!(r[0]["age"], 25);
    assert_eq!(r[1]["city"], "LA");
    assert_eq!(r[1]["age"], 30);
    assert_eq!(r[2]["city"], "NYC");
    assert_eq!(r[2]["age"], 30);
    assert_eq!(r[3]["city"], "NYC");
    assert_eq!(r[3]["age"], 35);
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
    let r = to_json(&q_paged);
    assert_eq!(r[0]["v"], 2);
    assert_eq!(r[1]["v"], 3);
    let info = q_info.unwrap();
    assert_eq!(info.total_count, Some(4));
}

#[test]
fn combined_with_dec_divergence_case() {
    // Dec("1.0") and Str("1.0") should dedup to one row (canonical-key),
    // then sort correctly among other values.
    // Canonical: Dec→String, so Dec("1.0")→"1.0" and Str("1.0")→"1.0" collide.
    // Dec("2.0")→"2.0", Dec("3.0")→"3.0".
    // Sorted lexicographically: "1.0" < "2.0" < "3.0".
    let qvs = vec![
        qv_map(&[("v", QueryValue::Dec("3.0".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Str("1.0".into()))]),
        qv_map(&[("v", QueryValue::Dec("1.0".parse().unwrap()))]),
        qv_map(&[("v", QueryValue::Dec("2.0".parse().unwrap()))]),
    ];

    let q_distinct = apply_distinct_qv(qvs);
    // Dec("3.0"), Str("1.0") [kept, Dec("1.0") deduped], Dec("2.0") → 3 rows
    assert_eq!(q_distinct.len(), 3);

    let mut q_sorted = q_distinct;
    apply_order_by_qv(&mut q_sorted, &OrderBy::asc("v"));

    let r = to_json(&q_sorted);
    // Lex order of string forms: "1.0" < "2.0" < "3.0"
    assert_eq!(r[0]["v"], "1.0");
    assert_eq!(r[1]["v"], "2.0");
    assert_eq!(r[2]["v"], "3.0");
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
    let r = to_json(&qvs);
    assert_eq!(r[0]["b"], false);
    assert_eq!(r[1]["b"], false);
    assert_eq!(r[2]["b"], true);
    assert_eq!(r[3]["b"], true);

    // DESC: true > false
    let mut qvs = vec![
        qv_map(&[("b", QueryValue::Bool(true))]),
        qv_map(&[("b", QueryValue::Bool(false))]),
    ];

    let order = OrderBy::desc("b");
    apply_order_by_qv(&mut qvs, &order);
    let r = to_json(&qvs);
    assert_eq!(r[0]["b"], true);
    assert_eq!(r[1]["b"], false);
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

    let result = apply_select_value(&records, &select, &interner);
    assert_eq!(result.len(), 4);

    let r = to_json(&result);
    assert_eq!(r[0]["name"], "Alice");
    assert_eq!(r[0]["age"], 30);
    assert_eq!(r[0]["city"], "NYC");
    assert_eq!(r[1]["name"], "Bob");
    assert_eq!(r[1]["age"], 25);
    assert_eq!(r[2]["name"], "Carol");
    assert_eq!(r[2]["age"], 35);
}

#[test]
fn select_value_all_returns_all_fields() {
    let interner = Interner::default();
    let records = make_test_records(&interner);
    let select = Select::all();

    let result = apply_select_value(&records, &select, &interner);
    assert_eq!(result.len(), 4);

    let r = to_json(&result);
    // All four fields present
    assert_eq!(r[0]["name"], "Alice");
    assert_eq!(r[0]["age"], 30);
    assert_eq!(r[0]["city"], "NYC");
    assert_eq!(r[0]["score"], 1.5);
}

#[test]
fn path_b_distinct_order_qv() {
    // Path B: apply_select_value -> distinct_qv -> order_by_qv
    let interner = Interner::default();
    let records = make_test_records(&interner);
    let select = Select::fields(["city", "age"]);

    let qv_result = apply_select_value(&records, &select, &interner);
    // 4 rows: {city:NYC,age:30},{city:LA,age:25},{city:NYC,age:35},{city:LA,age:25}
    // After distinct: {NYC,30},{LA,25},{NYC,35} → 3 distinct rows
    let q_distinct = apply_distinct_qv(qv_result);
    assert_eq!(q_distinct.len(), 3);

    let mut q_sorted = q_distinct;
    apply_order_by_qv(&mut q_sorted, &OrderBy::asc("age"));

    let r = to_json(&q_sorted);
    // sorted by age: 25, 30, 35
    assert_eq!(r[0]["age"], 25);
    assert_eq!(r[1]["age"], 30);
    assert_eq!(r[2]["age"], 35);
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

    let r = to_json(&result);

    // Groups sorted alphabetically: LA, NYC.
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["city"], "LA");
    assert_eq!(r[0]["cnt"], 2);
    assert_eq!(r[0]["sum_age"], 50);
    assert_eq!(r[0]["avg_age"], 25.0);
    assert_eq!(r[0]["min_age"], 25);
    assert_eq!(r[0]["max_age"], 25);
    assert_eq!(r[1]["city"], "NYC");
    assert_eq!(r[1]["cnt"], 2);
    assert_eq!(r[1]["sum_age"], 65);
    assert_eq!(r[1]["avg_age"], 32.5);
    assert_eq!(r[1]["min_age"], 30);
    assert_eq!(r[1]["max_age"], 35);
}

#[test]
fn aggregate_sum_float_serialisation() {
    // Sum of floats: the QV path must produce F64 that serialises identically
    // to json Number::from_f64. Total score = 1.5+2.5+3.5+0.5 = 8.0.
    let interner = Interner::default();
    let records = make_test_records(&interner);

    let select = Select {
        items: vec![select::sum("score", "total_score")],
        distinct: false,
    };

    let result = apply_aggregate_all(&to_bytes_records(&records), &select, &interner);
    assert_eq!(result.len(), 1);

    let qv_json = json::Value::from(result[0].clone());
    let qv_bytes = json::to_vec(&qv_json).unwrap();

    let total = 1.5 + 2.5 + 3.5 + 0.5; // = 8.0
    let expected_json = json::json!({"total_score": total});
    let expected_bytes = json::to_vec(&expected_json).unwrap();

    assert_eq!(
        qv_bytes, expected_bytes,
        "Sum(float) F64 serialisation must match json Number::from_f64"
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
    let r = to_json(&result);

    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["city"], "NYC");
    assert_eq!(r[0]["sum_age"], 65);
}

#[test]
fn aggregate_avg_float_serialisation() {
    // Avg produces F64 — must serialise identically to json Number::from_f64.
    // avg(score) = (1.5+2.5+3.5+0.5)/4 = 2.0
    let interner = Interner::default();
    let records = make_test_records(&interner);

    let select = Select {
        items: vec![select::avg("score", "avg_score")],
        distinct: false,
    };

    let result = apply_aggregate_all(&to_bytes_records(&records), &select, &interner);
    assert_eq!(result.len(), 1);

    let qv_json = json::Value::from(result[0].clone());
    let qv_bytes = json::to_vec(&qv_json).unwrap();

    let avg = (1.5 + 2.5 + 3.5 + 0.5) / 4.0; // = 2.0
    let expected_json = json::json!({"avg_score": avg});
    let expected_bytes = json::to_vec(&expected_json).unwrap();

    assert_eq!(
        qv_bytes, expected_bytes,
        "Avg F64 serialisation must match json Number::from_f64"
    );
}

#[test]
fn aggregate_count_as_int() {
    // Count produces Int(i64) — must serialise identically to json Number(u64).
    let interner = Interner::default();
    let records = make_test_records(&interner);

    let select = Select {
        items: vec![select::count_all("cnt")],
        distinct: false,
    };

    let result = apply_aggregate_all(&to_bytes_records(&records), &select, &interner);
    let qv_json = json::Value::from(result[0].clone());
    let qv_bytes = json::to_vec(&qv_json).unwrap();

    let expected_json = json::json!({"cnt": 4});
    let expected_bytes = json::to_vec(&expected_json).unwrap();

    assert_eq!(
        qv_bytes, expected_bytes,
        "Count serialisation must match json Number"
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

    let result = apply_aggregate_all(&to_bytes_records(&records), &select, &interner);
    let r = to_json(&result);

    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["cnt"], 4);
    assert_eq!(r[0]["sum_age"], 115);
    // avg_age = 115/4 = 28.75
    assert_eq!(r[0]["avg_age"], 28.75);
    assert_eq!(r[0]["min_age"], 25);
    assert_eq!(r[0]["max_age"], 35);
}
