//! Sum integer-overflow / exact-Big-lane regressions (former
//! `qv_postprocess_tests.rs`).

use shamir_types::types::value::QueryValue;

use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use crate::query::read::tests::query_results::select_order_page::qv_select_tests::intern;
use crate::query::read::{aggregate::AggAccum, AggFunc, AggregateField};

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
/// and must return the EXACT mathematical sum as `Big`, not a lossy `F64`
/// approximation.
///
/// CR-C5 (#780): before this fix, an `i64` overflow lifted Sum to the `f64`
/// lane (the same lossy path as a genuine float value), which could
/// silently round the exact integer total. `i64` overflow is still a
/// perfectly exact integer, just one that no longer fits in 64 bits — the
/// Preferred policy promotes it to an exact `BigInt` accumulator instead,
/// only falling back to `f64` once a genuinely non-integer value
/// (F64/fractional-Dec) enters the same aggregate.
#[test]
fn aggregate_sum_int_overflow_lifts_to_exact_big() {
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
    // True sum = 2 * half = i64::MAX + 1 = 9223372036854775808 — exact, not
    // rounded through f64 (which would also produce this same value here by
    // coincidence of rounding; the point is the EXACT type, verified more
    // sharply by the closer-pair test below).
    let result = acc.finish(&interner);
    assert_eq!(
        result,
        QueryValue::Big(num_bigint::BigInt::from(i64::MAX) + 1)
    );
}

/// Sharper version of the overflow test: two `i64::MAX`-adjacent values
/// whose sum, if rounded through `f64`, would collapse onto the SAME `f64`
/// as a neighboring exact sum — proving the exact `BigInt` lane actually
/// preserves precision `f64` could not.
#[test]
fn aggregate_sum_int_overflow_exact_not_f64_collapsed() {
    let interner = Interner::default();
    // i64::MAX + i64::MAX overflows i64; the two candidate totals
    // (2*i64::MAX) and (2*i64::MAX - 2) round to the SAME f64 (both inputs
    // are `> 2^53`), so an f64-based Sum could not tell these two scenarios
    // apart. The exact BigInt lane must.
    let records_a = vec![
        make_int_record(&interner, i64::MAX),
        make_int_record(&interner, i64::MAX),
    ];
    let records_b = vec![
        make_int_record(&interner, i64::MAX),
        make_int_record(&interner, i64::MAX - 1),
    ];
    let field = AggregateField::Field(vec!["v".into()]);

    let mut acc_a = AggAccum::new(AggFunc::Sum, &field, &interner);
    for rec in &records_a {
        acc_a.step(rec);
    }
    let mut acc_b = AggAccum::new(AggFunc::Sum, &field, &interner);
    for rec in &records_b {
        acc_b.step(rec);
    }

    let sum_a = acc_a.finish(&interner);
    let sum_b = acc_b.finish(&interner);
    assert_eq!(
        sum_a,
        QueryValue::Big(num_bigint::BigInt::from(i64::MAX) * 2)
    );
    assert_eq!(
        sum_b,
        QueryValue::Big(num_bigint::BigInt::from(i64::MAX) * 2 - 1)
    );
    assert_ne!(
        sum_a, sum_b,
        "exact sums one apart must not collapse to equal"
    );
}

/// Mixing an exact-overflow `Big` total with a genuine `F64` row must
/// transition the whole aggregate to the float lane (the documented,
/// unavoidable one-way transition) rather than silently staying "exact" with
/// the float contribution dropped.
#[test]
fn aggregate_sum_big_then_float_transitions_to_float_lane() {
    let interner = Interner::default();
    let mut map_a = new_map();
    map_a.insert(
        InternerKey::new(intern(&interner, "v")),
        InnerValue::Int(i64::MAX),
    );
    let mut map_b = new_map();
    map_b.insert(
        InternerKey::new(intern(&interner, "v")),
        InnerValue::Int(i64::MAX),
    );
    let mut map_c = new_map();
    map_c.insert(
        InternerKey::new(intern(&interner, "v")),
        InnerValue::F64(0.5),
    );
    let records = vec![
        InnerValue::Map(map_a),
        InnerValue::Map(map_b),
        InnerValue::Map(map_c),
    ];

    let field = AggregateField::Field(vec!["v".into()]);
    let mut acc = AggAccum::new(AggFunc::Sum, &field, &interner);
    for rec in &records {
        acc.step(rec);
    }
    let result = acc.finish(&interner);
    match result {
        QueryValue::F64(f) => {
            let expected = (i64::MAX as f64) * 2.0 + 0.5;
            assert!((f - expected).abs() < 1.0);
        }
        other => panic!("expected F64 once a genuine float value enters, got {other:?}"),
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
