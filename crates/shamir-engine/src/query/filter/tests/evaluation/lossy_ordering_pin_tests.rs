//! S3 contract pin (RV-FUNCLIB-5FA455F7): the lossy `F64` ↔ `Dec`/`Big`
//! comparison in `compare_values` remains ORDERING-only and UNCHANGED.
//! Equality consumers (filter Eq/Ne, DISTINCT / GROUP BY / FK) use the
//! ONE-rule canonical numeric key instead — under that rule every pair
//! asserted `Equal` below (F64(0.5)/Dec(0.5), F64(1e28)/Dec(1e28)) is now
//! NOT equal: non-integral `F64` and `Dec` stay in their own key classes.
//! These tests only pin that the ordering behaviour itself did not move.

use std::cmp::Ordering;

use num_bigint::BigInt;
use rust_decimal::Decimal;

use crate::query::filter::evaluation::semantics::resolve::compare_values;
use shamir_types::types::value::Value;

#[test]
fn rv_funclib_5fa455f7_f64_dec_big_ordering_still_lossy() {
    let dec_half = Value::<()>::Dec(Decimal::new(5, 1)); // 0.5 — 0.5_f64 is exact
    let dec_big = Value::<()>::Dec(Decimal::from_i128_with_scale(
        10_000_000_000_000_000_000_000_000_000,
        0,
    )); // 1e28
    let big = Value::<()>::Big(BigInt::from(10).pow(40u32));
    // Ordering (not equality) is the lossy surface and stays exactly as
    // before: the Dec/Big side is approximated through f64.
    assert_eq!(
        compare_values(&Value::<()>::F64(0.5), &dec_half),
        Some(Ordering::Equal)
    );
    assert_eq!(
        compare_values(&Value::<()>::F64(1e28), &dec_big),
        Some(Ordering::Equal)
    );
    // Strict orderings still resolve.
    assert_eq!(
        compare_values(&Value::<()>::F64(0.4), &dec_half),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare_values(&Value::<()>::F64(1e39), &big),
        Some(Ordering::Less)
    );
}
