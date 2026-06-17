//! Tests for the canonical cross-type total order.

use crate::compare::compare;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_types::types::value::QueryValue;
use std::cmp::Ordering;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn int(n: i64) -> QueryValue {
    QueryValue::Int(n)
}
fn f64v(f: f64) -> QueryValue {
    QueryValue::F64(f)
}
fn dec(s: &str) -> QueryValue {
    QueryValue::Dec(Decimal::from_str_exact(s).unwrap())
}
fn big(n: i64) -> QueryValue {
    QueryValue::Big(BigInt::from(n))
}
fn str_v(s: &str) -> QueryValue {
    QueryValue::Str(s.to_owned())
}
fn bool_v(b: bool) -> QueryValue {
    QueryValue::Bool(b)
}
fn bin(b: &[u8]) -> QueryValue {
    QueryValue::Bin(b.to_vec())
}
fn list(items: Vec<QueryValue>) -> QueryValue {
    QueryValue::List(items)
}

// ---------------------------------------------------------------------------
// Cross-subtype numeric equality: Int 5 == Dec 5.0 == F64 5.0 == Big 5
// ---------------------------------------------------------------------------

#[test]
fn numeric_cross_subtype_equality() {
    assert_eq!(compare(&int(5), &dec("5")), Ordering::Equal);
    assert_eq!(compare(&int(5), &f64v(5.0)), Ordering::Equal);
    assert_eq!(compare(&dec("5"), &f64v(5.0)), Ordering::Equal);
    assert_eq!(compare(&int(5), &big(5)), Ordering::Equal);
    assert_eq!(compare(&dec("5.0"), &big(5)), Ordering::Equal);
}

#[test]
fn numeric_cross_subtype_ordering() {
    assert_eq!(compare(&int(3), &dec("5")), Ordering::Less);
    assert_eq!(compare(&f64v(7.0), &int(3)), Ordering::Greater);
    assert_eq!(compare(&big(10), &dec("3")), Ordering::Greater);
}

// ---------------------------------------------------------------------------
// Cross-type rank ordering
// ---------------------------------------------------------------------------

#[test]
fn cross_type_rank_ordering() {
    // Null < Bool
    assert_eq!(compare(&QueryValue::Null, &bool_v(false)), Ordering::Less);
    // Bool < Number
    assert_eq!(compare(&bool_v(true), &int(0)), Ordering::Less);
    // Number < Str
    assert_eq!(compare(&int(999), &str_v("")), Ordering::Less);
    // Str < Bin
    assert_eq!(compare(&str_v("z"), &bin(b"")), Ordering::Less);
    // Bin < List
    assert_eq!(compare(&bin(b"\xff"), &list(vec![])), Ordering::Less);
}

// ---------------------------------------------------------------------------
// Str ordering
// ---------------------------------------------------------------------------

#[test]
fn str_lexicographic() {
    assert_eq!(compare(&str_v("abc"), &str_v("abd")), Ordering::Less);
    assert_eq!(compare(&str_v("abc"), &str_v("abc")), Ordering::Equal);
    assert_eq!(compare(&str_v("b"), &str_v("a")), Ordering::Greater);
}

// ---------------------------------------------------------------------------
// Bool ordering: false < true
// ---------------------------------------------------------------------------

#[test]
fn bool_ordering() {
    assert_eq!(compare(&bool_v(false), &bool_v(true)), Ordering::Less);
    assert_eq!(compare(&bool_v(true), &bool_v(true)), Ordering::Equal);
}

// ---------------------------------------------------------------------------
// List ordering
// ---------------------------------------------------------------------------

#[test]
fn list_elementwise() {
    let a = list(vec![int(1), int(2)]);
    let b = list(vec![int(1), int(3)]);
    assert_eq!(compare(&a, &b), Ordering::Less);
}

#[test]
fn list_prefix_shorter_is_less() {
    let a = list(vec![int(1)]);
    let b = list(vec![int(1), int(2)]);
    assert_eq!(compare(&a, &b), Ordering::Less);
}

// ---------------------------------------------------------------------------
// Bin ordering
// ---------------------------------------------------------------------------

#[test]
fn bin_byte_lexicographic() {
    assert_eq!(
        compare(&bin(b"\x00\x01"), &bin(b"\x00\x02")),
        Ordering::Less
    );
    assert_eq!(compare(&bin(b"\xff"), &bin(b"\xff")), Ordering::Equal);
}

// ---------------------------------------------------------------------------
// NaN behaviour
// ---------------------------------------------------------------------------

#[test]
fn nan_sorts_last_among_numerics() {
    assert_eq!(compare(&f64v(f64::NAN), &int(i64::MAX)), Ordering::Greater);
}

#[test]
fn nan_equals_nan() {
    assert_eq!(compare(&f64v(f64::NAN), &f64v(f64::NAN)), Ordering::Equal);
}

// ---------------------------------------------------------------------------
// Totality: compare(x, y) is reverse of compare(y, x)
// ---------------------------------------------------------------------------

#[test]
fn totality_reversal() {
    let values: Vec<QueryValue> = vec![
        QueryValue::Null,
        bool_v(false),
        bool_v(true),
        int(-1),
        int(0),
        int(5),
        f64v(3.15),
        dec("100"),
        big(42),
        f64v(f64::NAN),
        str_v("hello"),
        bin(b"data"),
        list(vec![int(1)]),
    ];
    for (i, a) in values.iter().enumerate() {
        for (j, b) in values.iter().enumerate() {
            let ab = compare(a, b);
            let ba = compare(b, a);
            assert_eq!(
                ab,
                ba.reverse(),
                "totality violated: compare(values[{}], values[{}]) = {:?}, \
                 reverse = {:?}",
                i,
                j,
                ab,
                ba,
            );
        }
    }
}
