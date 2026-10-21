//! Aggregate-pipeline QueryValue assertions (former
//! `qv_postprocess_tests.rs`, Stage E).


use rust_decimal::Decimal;
use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

use crate::query::filter::eval_context::FilterContext;

use crate::query::read::order::{apply_order_by_qv, apply_order_by_topk};
use crate::query::read::{
    aggregate::AggAccum, apply_aggregate_all, apply_group_by, AggFunc, AggregateField, GroupBy,
    OrderBy, OrderByItem, OrderDirection, Select,
};
use shamir_query_builder::select;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;
use crate::query::read::tests::query_results::select_order_page::qv_select_tests::{
    intern, make_test_records,
};
use crate::query::read::tests::query_results::select_order_page::helpers::to_bytes_records;
use crate::query::read::tests::query_results::select_order_page::qv_order_tests::qv_map;

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

/// FG-1/FG-6: `compare_values` (the engine-level comparison helper used by
/// `FilterNode::ValueCompare`, aggregate Min/Max, etc.) has explicit
/// `Big`↔`Int` arms that use the f64 fallback. This test confirms a
/// collection containing BOTH ordinary `Int` values and promoted `Big`
/// values compares correctly relative to each other via `compare_values`.
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

/// FG-6 fix: the ORDER BY path (`apply_order_by_qv`) now has an
/// `I64`/`F64`/`Dec`↔`Big` cross-type arm in `QvSortKey` (mirroring
/// `compare_values`'s existing arms), so mixed Int+Big rows sort NUMERICALLY
/// instead of falling to the `_ => Equal` arbitrary-order arm that preserved
/// insertion order. Exercises the real `apply_order_by_qv` entry point (not
/// just the comparator function directly).
#[test]
fn order_by_mixed_int_and_big_sorts_numerically() {
    use num_bigint::BigInt;

    // Insertion order is deliberately NOT sorted, so a no-op "_ => Equal"
    // comparator would leave it unchanged.
    let mut qvs = vec![
        qv_map(&[("v", QueryValue::Big(BigInt::from(u64::MAX)))]), // ~1.8e19
        qv_map(&[("v", QueryValue::Int(100))]),
        qv_map(&[(
            "v",
            QueryValue::Big(BigInt::from(i64::MAX as u64 + 1)), // ~9.2e18
        )]),
        qv_map(&[("v", QueryValue::Int(50))]),
    ];

    apply_order_by_qv(&mut qvs, &OrderBy::asc("v"));

    // Numeric ascending: Int(50) < Int(100) < Big(i64::MAX+1) < Big(u64::MAX).
    assert_eq!(qvs[0]["v"], QueryValue::Int(50));
    assert_eq!(qvs[1]["v"], QueryValue::Int(100));
    assert_eq!(
        qvs[2]["v"],
        QueryValue::Big(BigInt::from(i64::MAX as u64 + 1))
    );
    assert_eq!(qvs[3]["v"], QueryValue::Big(BigInt::from(u64::MAX)));
}

/// FG-6: DESC direction + Big/Big exact comparison (not just cross-type
/// against Int) — two Big values that are close in magnitude but not equal
/// must still sort correctly (BigInt: Ord is exact, unlike the f64 fallback
/// used for cross-type comparisons).
#[test]
fn order_by_big_vs_big_desc_exact() {
    use num_bigint::BigInt;

    let huge = BigInt::from(u64::MAX) * BigInt::from(1_000_000);
    let bigger = &huge + BigInt::from(1);

    let mut qvs = vec![
        qv_map(&[("v", QueryValue::Big(bigger.clone()))]),
        qv_map(&[("v", QueryValue::Big(huge.clone()))]),
    ];

    apply_order_by_qv(&mut qvs, &OrderBy::desc("v"));

    assert_eq!(qvs[0]["v"], QueryValue::Big(bigger));
    assert_eq!(qvs[1]["v"], QueryValue::Big(huge));
}

