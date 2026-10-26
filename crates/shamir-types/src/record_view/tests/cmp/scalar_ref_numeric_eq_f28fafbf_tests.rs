//! RV-TYPES-F28FAFBF regression tests: a stored `Dec`/`Big` projects to its
//! decimal STRING on the v1 wire, so `scalar_ref_numeric_eq` must match
//! `ScalarRef::Str` against typed `QueryValue::Dec`/`Big` literals via the
//! canonical `NumericKey` (THE ONE RULE, `types::numeric_key`).

use rust_decimal::Decimal;
use std::str::FromStr;

use crate::record_view::scalar_ref::scalar_ref_numeric_eq;
use crate::record_view::ScalarRef;
use crate::types::value::QueryValue;

#[test]
fn f28fafbf_stored_str_matches_equal_dec_literal() {
    // "1.50" is the projected wire form of Dec(1.50) ⇒ equal literal.
    let stored = ScalarRef::Str("1.50");
    assert!(scalar_ref_numeric_eq(
        stored,
        &QueryValue::Dec(Decimal::new(150, 2))
    ));
}

#[test]
fn f28fafbf_stored_str_does_not_match_different_dec_literal() {
    let stored = ScalarRef::Str("1.50");
    assert!(!scalar_ref_numeric_eq(
        stored,
        &QueryValue::Dec(Decimal::new(151, 2)),
    ));
}

/// The stored string is parsed EXACTLY: a value needing more digits than a
/// `Decimal` holds is not rounded into an equal-looking typed literal.
#[test]
fn f28fafbf_stored_str_is_parsed_exactly_not_rounded() {
    let stored = ScalarRef::Str("1.00000000000000000000000000001");
    assert!(!scalar_ref_numeric_eq(
        stored,
        &QueryValue::Dec(Decimal::from(1)),
    ));
}

#[test]
fn f28fafbf_stored_str_matches_big_literal_beyond_i64() {
    let big = "123456789012345678901234567890";
    assert!(scalar_ref_numeric_eq(
        ScalarRef::Str(big),
        &QueryValue::Big(num_bigint::BigInt::from_str(big).unwrap()),
    ));
}

#[test]
fn f28fafbf_non_numeric_stored_str_vs_dec_is_false() {
    assert!(!scalar_ref_numeric_eq(
        ScalarRef::Str("hello"),
        &QueryValue::Dec(Decimal::new(150, 2)),
    ));
    assert!(!scalar_ref_numeric_eq(
        ScalarRef::Str("1.5.5"),
        &QueryValue::Dec(Decimal::new(150, 2)),
    ));
}

#[test]
fn f28fafbf_stored_str_vs_str_equality_unchanged() {
    // Existing Str-vs-Str arm must be untouched by the typed-RHS fix.
    assert!(scalar_ref_numeric_eq(
        ScalarRef::Str("1.50"),
        &QueryValue::Str("1.50".into())
    ));
    assert!(!scalar_ref_numeric_eq(
        ScalarRef::Str("1.50"),
        &QueryValue::Str("1.51".into())
    ));
}

#[test]
fn f28fafbf_stored_str_vs_int_still_not_equal() {
    // A projected decimal string is NOT a stored Int: no cross-family match.
    assert!(!scalar_ref_numeric_eq(
        ScalarRef::Str("1"),
        &QueryValue::Int(1)
    ));
}
