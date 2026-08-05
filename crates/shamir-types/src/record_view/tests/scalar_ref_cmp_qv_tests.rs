//! Parity tests for [`scalar_ref_cmp_qv`] — the `QueryValue` twin of
//! [`scalar_ref_cmp`] (see `scalar_ref_cmp_tests.rs`). Focused on the `Bin`
//! arm added alongside `scalar_ref_cmp`'s (#983 fix) — the rest of the
//! cross-type matrix is already covered by the `InnerValue` parity tests and
//! `scalar_ref_cmp_qv` is documented to order identically for every scalar
//! arm.

use std::cmp::Ordering;

use crate::record_view::scalar_ref::scalar_ref_cmp_qv;
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
