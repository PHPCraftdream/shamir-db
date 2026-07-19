//! Tests for the canonical cross-type total order.

use crate::compare::compare;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_types::types::common::{new_map, new_set};
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
fn set(items: Vec<QueryValue>) -> QueryValue {
    let mut s = new_set();
    for i in items {
        s.insert(i);
    }
    QueryValue::Set(s)
}
fn map(pairs: Vec<(&str, QueryValue)>) -> QueryValue {
    let mut m = new_map();
    for (k, v) in pairs {
        m.insert(k.to_owned(), v);
    }
    QueryValue::Map(m)
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
// Set / Map structural comparison (was length-only before the fix)
// ---------------------------------------------------------------------------

#[test]
fn set_different_entries_same_length_not_equal() {
    // Two structurally-DIFFERENT single-entry Sets of equal length must NOT
    // compare Equal.
    let a = set(vec![int(1)]);
    let b = set(vec![int(2)]);
    assert_ne!(compare(&a, &b), Ordering::Equal);
}

#[test]
fn set_identical_different_insertion_order_equal() {
    // Two structurally-identical Sets built with different insertion order
    // must compare Equal.
    let a = set(vec![int(3), int(1), int(2)]);
    let b = set(vec![int(1), int(2), int(3)]);
    assert_eq!(compare(&a, &b), Ordering::Equal);
}

#[test]
fn set_nested_structural_equality() {
    // Nested sets: { {1, 2} } == { {2, 1} } (inner set canonicalized too).
    let a = set(vec![set(vec![int(1), int(2)])]);
    let b = set(vec![set(vec![int(2), int(1)])]);
    assert_eq!(compare(&a, &b), Ordering::Equal);
}

#[test]
fn map_different_entries_same_length_not_equal() {
    // Two structurally-DIFFERENT single-entry Maps of equal length must NOT
    // compare Equal.
    let a = map(vec![("a", int(1))]);
    let b = map(vec![("b", int(2))]);
    assert_ne!(compare(&a, &b), Ordering::Equal);
}

#[test]
fn map_identical_different_insertion_order_equal() {
    // Two structurally-identical Maps built with different insertion order
    // must compare Equal.
    let a = map(vec![("c", int(3)), ("a", int(1)), ("b", int(2))]);
    let b = map(vec![("a", int(1)), ("b", int(2)), ("c", int(3))]);
    assert_eq!(compare(&a, &b), Ordering::Equal);
}

#[test]
fn map_same_keys_different_values_not_equal() {
    // Same keys, different values → not Equal.
    let a = map(vec![("k", int(1))]);
    let b = map(vec![("k", int(2))]);
    assert_ne!(compare(&a, &b), Ordering::Equal);
}

#[test]
fn map_nested_structural_equality() {
    // Nested maps: {"outer": {"y": 2, "x": 1}} == {"outer": {"x": 1, "y": 2}}.
    let a = map(vec![("outer", map(vec![("y", int(2)), ("x", int(1))]))]);
    let b = map(vec![("outer", map(vec![("x", int(1)), ("y", int(2))]))]);
    assert_eq!(compare(&a, &b), Ordering::Equal);
}

#[test]
fn set_map_transitivity() {
    // Transitivity: if a < b and b < c then a < c.
    let a = set(vec![int(1)]);
    let b = set(vec![int(1), int(2)]);
    let c = set(vec![int(1), int(2), int(3)]);
    let ab = compare(&a, &b);
    let bc = compare(&b, &c);
    let ac = compare(&a, &c);
    assert_eq!(ab, Ordering::Less);
    assert_eq!(bc, Ordering::Less);
    assert_eq!(ac, Ordering::Less);

    let ma = map(vec![("a", int(1))]);
    let mb = map(vec![("a", int(1)), ("b", int(2))]);
    let mc = map(vec![("a", int(1)), ("b", int(2)), ("c", int(3))]);
    assert_eq!(compare(&ma, &mb), Ordering::Less);
    assert_eq!(compare(&mb, &mc), Ordering::Less);
    assert_eq!(compare(&ma, &mc), Ordering::Less);
}

#[test]
fn set_map_totality_reversal() {
    // Antisymmetry: compare(a, b) must be the reverse of compare(b, a).
    let pairs: [(QueryValue, QueryValue); 4] = [
        (set(vec![int(1)]), set(vec![int(2)])),
        (set(vec![int(1), int(2)]), set(vec![int(2), int(3)])),
        (map(vec![("a", int(1))]), map(vec![("b", int(2))])),
        (map(vec![("k", int(1))]), map(vec![("k", int(2))])),
    ];
    for (i, (a, b)) in pairs.iter().enumerate() {
        let ab = compare(a, b);
        let ba = compare(b, a);
        assert_eq!(
            ab,
            ba.reverse(),
            "totality violated for pair {}: {:?} vs {:?}",
            i,
            ab,
            ba
        );
    }
}

// ---------------------------------------------------------------------------
// List regression: compare on two structurally-identical Lists still works
// (List path untouched by this fix)
// ---------------------------------------------------------------------------

#[test]
fn list_structural_equality_unchanged() {
    let a = list(vec![int(1), str_v("x")]);
    let b = list(vec![int(1), str_v("x")]);
    assert_eq!(compare(&a, &b), Ordering::Equal);
    // List is NOT canonicalized — different order is NOT equal (unlike Set).
    let c = list(vec![str_v("x"), int(1)]);
    assert_ne!(compare(&a, &c), Ordering::Equal);
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
