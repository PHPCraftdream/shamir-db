//! Parity tests for [`scalar_ref_cmp_qv`] — the `QueryValue` twin of
//! [`scalar_ref_cmp`] (see `scalar_ref_cmp_tests.rs`). Focused on the `Bin`
//! arm added alongside `scalar_ref_cmp`'s (#983 fix) — the rest of the
//! cross-type matrix is already covered by the `InnerValue` parity tests and
//! `scalar_ref_cmp_qv` is documented to order identically for every scalar
//! arm.

use std::cmp::Ordering;

use num_bigint::BigInt;

use crate::record_view::scalar_ref::{scalar_ref_cmp_qv, scalar_ref_numeric_eq};
use crate::record_view::ScalarRef;
use crate::types::value::QueryValue;

#[test]
fn bin_bin_equal_qv() {
    assert_eq!(
        scalar_ref_cmp_qv(ScalarRef::Bin(&[1, 2, 3]), &QueryValue::Bin(vec![1, 2, 3])),
        Some(Ordering::Equal),
    );
}

#[test]
fn bin_bin_not_equal_qv() {
    // [1, 2, 3] < [1, 2, 4] lexicographically (last byte differs).
    assert_eq!(
        scalar_ref_cmp_qv(ScalarRef::Bin(&[1, 2, 3]), &QueryValue::Bin(vec![1, 2, 4])),
        Some(Ordering::Less),
    );
    // [1, 2, 4] > [1, 2, 3] lexicographically.
    assert_eq!(
        scalar_ref_cmp_qv(ScalarRef::Bin(&[1, 2, 4]), &QueryValue::Bin(vec![1, 2, 3])),
        Some(Ordering::Greater),
    );
}

#[test]
fn bin_vs_mismatched_family_returns_none_qv() {
    // Bin vs Str: mismatched type families still return None.
    assert_eq!(
        scalar_ref_cmp_qv(ScalarRef::Bin(&[1, 2, 3]), &QueryValue::Str("abc".into())),
        None,
    );
}

// ── scalar_ref_numeric_eq: EXACT equality (RV-FUNCLIB-5FA455F7, S2) ─────────

/// ONE rule (round 3): a non-integral `F64` and a non-integral `Dec` stay
/// in their OWN key classes — `F64(0.5) != Dec(0.5)` AND
/// `F64(0.1) != Dec("0.1")`; only an INTEGRAL `F64` equals the equal
/// `Dec`. The old `scalar_ref_cmp_qv == Equal` path said F64(0.5) ==
/// Dec(0.5) (lossy `dec_to_f64`) — ordering keeps that, equality does not.
#[test]
fn rv_funclib_5fa455f7_scalar_ref_numeric_eq_f64_dec_exact_qv() {
    let half = rust_decimal::Decimal::new(5, 1);
    let tenth = rust_decimal::Decimal::new(1, 1);
    assert!(!scalar_ref_numeric_eq(
        ScalarRef::F64(0.5),
        &QueryValue::Dec(half)
    ));
    assert!(!scalar_ref_numeric_eq(
        ScalarRef::F64(0.1),
        &QueryValue::Dec(tenth)
    ));
    // Integral F64 still equals the integral Dec (same integer key).
    assert!(scalar_ref_numeric_eq(
        ScalarRef::F64(1.0),
        &QueryValue::Dec(rust_decimal::Decimal::from(1))
    ));
    // Ordering stays lossy by design (S3) — pin the difference: F64(0.5)
    // ORDERS equal to Dec(0.5) but is NOT equal to it.
    assert_eq!(
        scalar_ref_cmp_qv(ScalarRef::F64(0.5), &QueryValue::Dec(half)),
        Some(Ordering::Equal)
    );
}

/// Exact Int↔Big beyond `2^53`: `F64(2^53)` equals `Big(2^53)` but NOT
/// `Big(2^53+1)` (the old lossy path collapsed both into the rounded
/// `f64(2^53)`). `Int(1)` equals `Big(1)`; NaN matches nothing.
#[test]
fn rv_funclib_5fa455f7_scalar_ref_numeric_eq_int_big_exact_qv() {
    let two_pow_53 = 9007199254740992i64;
    assert!(scalar_ref_numeric_eq(
        ScalarRef::F64(two_pow_53 as f64),
        &QueryValue::Big(BigInt::from(two_pow_53))
    ));
    assert!(!scalar_ref_numeric_eq(
        ScalarRef::F64(two_pow_53 as f64),
        &QueryValue::Big(BigInt::from(two_pow_53 + 1))
    ));
    assert!(scalar_ref_numeric_eq(
        ScalarRef::Int(1),
        &QueryValue::Big(BigInt::from(1))
    ));
    assert!(!scalar_ref_numeric_eq(
        ScalarRef::Int(1),
        &QueryValue::Big(BigInt::from(2))
    ));
}

/// NaN is never equal to anything — not to itself, not to an Int, not to
/// NaN. (Under the old `scalar_ref_cmp_qv` path NaN yielded `None`, which
/// callers turned into "not equal" for Eq but the semantics are now first
/// class in the predicate.)
#[test]
fn rv_funclib_5fa455f7_scalar_ref_numeric_eq_nan_matches_nothing_qv() {
    assert!(!scalar_ref_numeric_eq(
        ScalarRef::F64(f64::NAN),
        &QueryValue::F64(f64::NAN)
    ));
    assert!(!scalar_ref_numeric_eq(
        ScalarRef::F64(f64::NAN),
        &QueryValue::Int(0)
    ));
    assert!(!scalar_ref_numeric_eq(
        ScalarRef::Int(1),
        &QueryValue::F64(f64::NAN)
    ));
}

/// ORDERING regression (QV twin): Int↔Big ordering is EXACT via
/// `BigInt: Ord` (RV-FUNCLIB-5FA455F7, S2) — the `(a as f64)` cast equated
/// distinct integers beyond 2^53 (all three orderings came back Equal).
#[test]
fn rv_funclib_5fa455f7_scalar_ref_cmp_qv_int_big_ordering_exact_beyond_2p53() {
    let i = 9007199254740993i64; // 2^53 + 1
    assert_eq!(
        scalar_ref_cmp_qv(ScalarRef::Int(i), &QueryValue::Big(BigInt::from(i))),
        Some(Ordering::Equal)
    );
    assert_eq!(
        scalar_ref_cmp_qv(
            ScalarRef::Int(i),
            &QueryValue::Big(BigInt::from(i - 1)) // 2^53
        ),
        Some(Ordering::Greater)
    );
    assert_eq!(
        scalar_ref_cmp_qv(ScalarRef::Int(i), &QueryValue::Big(BigInt::from(i + 1))),
        Some(Ordering::Less)
    );
}
