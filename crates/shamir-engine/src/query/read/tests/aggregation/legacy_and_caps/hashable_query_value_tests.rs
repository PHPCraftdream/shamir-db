use num_bigint::BigInt;
use rust_decimal::Decimal;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::query::read::hashable_query_value::HashableQueryValue;
use shamir_types::types::value::QueryValue;

fn hash_of(qv: &QueryValue) -> u64 {
    let mut h = DefaultHasher::new();
    HashableQueryValue(qv).hash(&mut h);
    h.finish()
}

fn eq(a: &QueryValue, b: &QueryValue) -> bool {
    HashableQueryValue(a) == HashableQueryValue(b)
}

// ── Dec / Big vs Str: separate classes (exact numeric classes, D1-S2) ────
//
// DISTINCT/GROUP BY bucket numbers by the shared canonical NumericKey.
// An exact numeric class cannot also equal a string form (Big(1) == Int(1)
// would make Str("1") == Int(1) transitively), so a Dec/Big no longer shares
// a class with its decimal string. Stored Big/Dec reach these paths as
// strings until the D2 wire projection; see the numeric_key module docs.

#[test]
fn dec_and_str_are_separate_classes() {
    let dec = QueryValue::Dec("1.5".parse::<Decimal>().unwrap());
    let str = QueryValue::Str("1.5".to_string());
    assert!(!eq(&dec, &str), "Dec and Str form must not share a class");
    assert!(!eq(&str, &dec), "symmetry");
}

#[test]
fn dec_str_different_not_eq() {
    let dec = QueryValue::Dec("1.5".parse::<Decimal>().unwrap());
    let str = QueryValue::Str("2.5".to_string());
    assert!(!eq(&dec, &str));
}

#[test]
fn dec_equal_values_share_hash_and_class() {
    let a = QueryValue::Dec("1.50".parse::<Decimal>().unwrap());
    let b = QueryValue::Dec("1.5".parse::<Decimal>().unwrap());
    assert_eq!(hash_of(&a), hash_of(&b));
    assert!(eq(&a, &b));
}

#[test]
fn big_and_str_are_separate_classes() {
    let big = QueryValue::Big(BigInt::from(42));
    let str = QueryValue::Str("42".to_string());
    assert!(!eq(&big, &str), "Big and Str form must not share a class");
    assert!(!eq(&str, &big), "symmetry");
}

#[test]
fn big_equals_int_and_integral_dec() {
    let big = QueryValue::Big(BigInt::from(42));
    let int = QueryValue::Int(42);
    let dec = QueryValue::Dec(Decimal::from(42));
    assert_eq!(hash_of(&big), hash_of(&int));
    assert_eq!(hash_of(&big), hash_of(&dec));
    assert!(eq(&big, &int));
    assert!(eq(&int, &dec));
}

// ── F64 finite hashes by bits ────────────────────────────────────────────

#[test]
fn f64_finite_eq_by_bits() {
    let a = QueryValue::F64(1.5);
    let b = QueryValue::F64(1.5);
    let c = QueryValue::F64(2.5);
    assert_eq!(hash_of(&a), hash_of(&b));
    assert!(eq(&a, &b));
    assert!(!eq(&a, &c));
}

#[test]
fn f64_nonfinite_are_not_strings() {
    // NaN never equals anything (per-occurrence NaN is applied by the
    // DISTINCT/GROUP BY drivers); infinities keep their own raw-bits class
    // and no longer alias the strings "NaN" / "inf".
    let nan = QueryValue::F64(f64::NAN);
    let nan_str = QueryValue::Str("NaN".to_string());
    assert!(!eq(&nan, &nan_str));

    let inf = QueryValue::F64(f64::INFINITY);
    let inf_str = QueryValue::Str("inf".to_string());
    assert!(!eq(&inf, &inf_str));
    assert!(eq(&inf, &QueryValue::F64(f64::INFINITY)));
    assert!(!eq(&inf, &QueryValue::F64(f64::NEG_INFINITY)));
}

// ── Bin dedup ────────────────────────────────────────────────────────────

#[test]
fn bin_same_bytes_eq() {
    let a = QueryValue::Bin(vec![1, 2, 3]);
    let b = QueryValue::Bin(vec![1, 2, 3]);
    assert_eq!(hash_of(&a), hash_of(&b));
    assert!(eq(&a, &b));
}

#[test]
fn bin_different_bytes_not_eq() {
    let a = QueryValue::Bin(vec![1, 2]);
    let b = QueryValue::Bin(vec![1, 3]);
    assert!(!eq(&a, &b));
}

// ── Null ─────────────────────────────────────────────────────────────────

#[test]
fn null_eq_null() {
    assert!(eq(&QueryValue::Null, &QueryValue::Null));
    assert!(!eq(&QueryValue::Null, &QueryValue::Int(0)));
}

// ── Int distinct from String ─────────────────────────────────────────────

#[test]
fn int_not_eq_str() {
    let int = QueryValue::Int(42);
    let str = QueryValue::Str("42".to_string());
    // Int → Number; Str → String; different canonical forms.
    assert!(!eq(&int, &str));
    assert_ne!(hash_of(&int), hash_of(&str));
}

// ── Map insertion-order eq ───────────────────────────────────────────────

#[test]
fn map_eq_same_order() {
    use shamir_types::types::common::new_map_wc;
    let mut m1 = new_map_wc(2);
    m1.insert("a".to_string(), QueryValue::Int(1));
    m1.insert("b".to_string(), QueryValue::Int(2));
    let mut m2 = new_map_wc(2);
    m2.insert("a".to_string(), QueryValue::Int(1));
    m2.insert("b".to_string(), QueryValue::Int(2));
    assert!(eq(&QueryValue::Map(m1), &QueryValue::Map(m2)));
}
