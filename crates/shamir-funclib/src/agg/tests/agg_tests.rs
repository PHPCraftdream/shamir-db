//! Tests for every built-in aggregator — happy-path, empty-input, and
//! cross-type min/max.

use crate::agg::{self, AggRegistry};
use rust_decimal::Decimal;
use shamir_types::types::value::QueryValue;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn registry() -> AggRegistry {
    let mut r = AggRegistry::new();
    agg::register(&mut r);
    r
}

fn run(name: &str, values: &[QueryValue]) -> Result<QueryValue, crate::registry::ScalarError> {
    let reg = registry();
    let mut a = reg.make(name).expect("aggregator not found");
    for v in values {
        a.accumulate(v)?;
    }
    a.finalize()
}

fn int(n: i64) -> QueryValue {
    QueryValue::Int(n)
}
fn f64v(f: f64) -> QueryValue {
    QueryValue::F64(f)
}
fn dec(s: &str) -> QueryValue {
    QueryValue::Dec(Decimal::from_str_exact(s).unwrap())
}
fn str_v(s: &str) -> QueryValue {
    QueryValue::Str(s.to_owned())
}
fn bool_v(b: bool) -> QueryValue {
    QueryValue::Bool(b)
}

// ---------------------------------------------------------------------------
// count
// ---------------------------------------------------------------------------

#[test]
fn count_basic() {
    let r = run("count", &[int(1), int(2), QueryValue::Null, int(3)]).unwrap();
    assert_eq!(r, int(3));
}

#[test]
fn count_empty() {
    assert_eq!(run("count", &[]).unwrap(), int(0));
}

// ---------------------------------------------------------------------------
// count_distinct
// ---------------------------------------------------------------------------

#[test]
fn count_distinct_basic() {
    // Int 5 and Dec 5 are equal by compare, so count_distinct should be 2.
    let r = run(
        "count_distinct",
        &[int(5), dec("5"), int(3), QueryValue::Null],
    )
    .unwrap();
    assert_eq!(r, int(2));
}

#[test]
fn count_distinct_empty() {
    assert_eq!(run("count_distinct", &[]).unwrap(), int(0));
}

// ---------------------------------------------------------------------------
// sum
// ---------------------------------------------------------------------------

#[test]
fn sum_basic() {
    let r = run("sum", &[int(1), int(2), int(3)]).unwrap();
    assert_eq!(r, QueryValue::Dec(Decimal::from(6)));
}

#[test]
fn sum_empty() {
    assert_eq!(run("sum", &[]).unwrap(), int(0));
}

#[test]
fn sum_type_mismatch() {
    assert_eq!(run("sum", &[str_v("x")]).unwrap_err().code, "type_mismatch");
}

// ---------------------------------------------------------------------------
// avg
// ---------------------------------------------------------------------------

#[test]
fn avg_basic() {
    let r = run("avg", &[int(2), int(4), int(6)]).unwrap();
    assert_eq!(r, QueryValue::Dec(Decimal::from(4)));
}

#[test]
fn avg_empty() {
    assert_eq!(run("avg", &[]).unwrap_err().code, "empty");
}

// ---------------------------------------------------------------------------
// min / max — CROSS-TYPE test
// ---------------------------------------------------------------------------

#[test]
fn min_cross_type() {
    // Bool(true) has rank 1, Int(5) rank 2, Str("a") rank 3.
    // So min = Bool(true), max = Str("a").
    let r = run("min", &[int(5), str_v("a"), bool_v(true)]).unwrap();
    assert_eq!(r, bool_v(true));
}

#[test]
fn max_cross_type() {
    let r = run("max", &[int(5), str_v("a"), bool_v(true)]).unwrap();
    assert_eq!(r, str_v("a"));
}

#[test]
fn min_empty() {
    assert_eq!(run("min", &[]).unwrap_err().code, "empty");
}

#[test]
fn max_empty() {
    assert_eq!(run("max", &[]).unwrap_err().code, "empty");
}

#[test]
fn min_numeric() {
    let r = run("min", &[int(10), dec("3.5"), f64v(7.0)]).unwrap();
    assert_eq!(r, dec("3.5"));
}

// ---------------------------------------------------------------------------
// median
// ---------------------------------------------------------------------------

#[test]
fn median_odd() {
    let r = run("median", &[int(3), int(1), int(5)]).unwrap();
    assert_eq!(r, int(3));
}

#[test]
fn median_even() {
    // Even count: lower-median (index n/2 - 1 after sort).
    let r = run("median", &[int(1), int(2), int(3), int(4)]).unwrap();
    assert_eq!(r, int(2));
}

#[test]
fn median_empty() {
    assert_eq!(run("median", &[]).unwrap_err().code, "empty");
}

// ---------------------------------------------------------------------------
// stddev / variance
// ---------------------------------------------------------------------------

#[test]
fn variance_basic() {
    // [2, 4, 4, 4, 5, 5, 7, 9] -> mean=5, variance=4
    let vals: Vec<QueryValue> = vec![
        int(2),
        int(4),
        int(4),
        int(4),
        int(5),
        int(5),
        int(7),
        int(9),
    ];
    let r = run("variance", &vals).unwrap();
    assert_eq!(r, QueryValue::Dec(Decimal::from(4)));
}

#[test]
fn stddev_basic() {
    let vals: Vec<QueryValue> = vec![
        int(2),
        int(4),
        int(4),
        int(4),
        int(5),
        int(5),
        int(7),
        int(9),
    ];
    let r = run("stddev", &vals).unwrap();
    // stddev = 2.0
    assert_eq!(r, QueryValue::Dec(Decimal::from(2)));
}

#[test]
fn variance_empty() {
    assert_eq!(run("variance", &[]).unwrap_err().code, "empty");
}

#[test]
fn stddev_empty() {
    assert_eq!(run("stddev", &[]).unwrap_err().code, "empty");
}

// ---------------------------------------------------------------------------
// percentile
// ---------------------------------------------------------------------------

#[test]
fn percentile_default_is_median() {
    let r = run("percentile", &[int(1), int(2), int(3), int(4), int(5)]).unwrap();
    // p=0.5 on 5 elements: ceil(0.5*5)=3, index=2 -> value 3
    assert_eq!(r, int(3));
}

#[test]
fn percentile_empty() {
    assert_eq!(run("percentile", &[]).unwrap_err().code, "empty");
}

// ---------------------------------------------------------------------------
// first / last
// ---------------------------------------------------------------------------

#[test]
fn first_basic() {
    let r = run("first", &[QueryValue::Null, int(10), int(20)]).unwrap();
    assert_eq!(r, int(10));
}

#[test]
fn last_basic() {
    let r = run("last", &[int(10), int(20), QueryValue::Null]).unwrap();
    assert_eq!(r, int(20));
}

#[test]
fn first_empty() {
    assert_eq!(run("first", &[]).unwrap_err().code, "empty");
}

#[test]
fn last_empty() {
    assert_eq!(run("last", &[]).unwrap_err().code, "empty");
}

// ---------------------------------------------------------------------------
// string_agg
// ---------------------------------------------------------------------------

#[test]
fn string_agg_basic() {
    let r = run("string_agg", &[str_v("a"), str_v("b"), str_v("c")]).unwrap();
    assert_eq!(r, str_v("a,b,c"));
}

#[test]
fn string_agg_empty() {
    assert_eq!(run("string_agg", &[]).unwrap(), str_v(""));
}

#[test]
fn string_agg_type_mismatch() {
    assert_eq!(
        run("string_agg", &[int(1)]).unwrap_err().code,
        "type_mismatch"
    );
}

// ---------------------------------------------------------------------------
// array_agg (includes Nulls)
// ---------------------------------------------------------------------------

#[test]
fn array_agg_basic() {
    let r = run("array_agg", &[int(1), QueryValue::Null, str_v("x")]).unwrap();
    assert_eq!(
        r,
        QueryValue::List(vec![int(1), QueryValue::Null, str_v("x")])
    );
}

#[test]
fn array_agg_empty() {
    assert_eq!(run("array_agg", &[]).unwrap(), QueryValue::List(vec![]));
}

// ---------------------------------------------------------------------------
// bool_and / bool_or
// ---------------------------------------------------------------------------

#[test]
fn bool_and_basic() {
    assert_eq!(
        run("bool_and", &[bool_v(true), bool_v(true)]).unwrap(),
        bool_v(true)
    );
    assert_eq!(
        run("bool_and", &[bool_v(true), bool_v(false)]).unwrap(),
        bool_v(false)
    );
}

#[test]
fn bool_and_empty() {
    assert_eq!(run("bool_and", &[]).unwrap(), bool_v(true));
}

#[test]
fn bool_or_basic() {
    assert_eq!(
        run("bool_or", &[bool_v(false), bool_v(true)]).unwrap(),
        bool_v(true)
    );
    assert_eq!(
        run("bool_or", &[bool_v(false), bool_v(false)]).unwrap(),
        bool_v(false)
    );
}

#[test]
fn bool_or_empty() {
    assert_eq!(run("bool_or", &[]).unwrap(), bool_v(false));
}

// ---------------------------------------------------------------------------
// mode
// ---------------------------------------------------------------------------

#[test]
fn mode_basic() {
    let r = run("mode", &[int(1), int(2), int(2), int(3)]).unwrap();
    assert_eq!(r, int(2));
}

#[test]
fn mode_empty() {
    assert_eq!(run("mode", &[]).unwrap_err().code, "empty");
}

// ---------------------------------------------------------------------------
// range
// ---------------------------------------------------------------------------

#[test]
fn range_basic() {
    let r = run("range", &[int(3), int(10), int(1)]).unwrap();
    assert_eq!(r, QueryValue::Dec(Decimal::from(9)));
}

#[test]
fn range_empty() {
    assert_eq!(run("range", &[]).unwrap(), int(0));
}

// ---------------------------------------------------------------------------
// Registry coverage
// ---------------------------------------------------------------------------

#[test]
fn all_aggregators_registered() {
    let reg = registry();
    let expected = vec![
        "count",
        "count_distinct",
        "sum",
        "avg",
        "min",
        "max",
        "median",
        "stddev",
        "variance",
        "percentile",
        "first",
        "last",
        "string_agg",
        "array_agg",
        "bool_and",
        "bool_or",
        "mode",
        "range",
    ];
    let names = reg.names();
    for name in &expected {
        assert!(names.contains(name), "missing aggregator: {}", name);
    }
    assert_eq!(names.len(), expected.len());
}
