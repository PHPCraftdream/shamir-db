//! Parity tests for [`scalar_ref_cmp`] — proves it mirrors `compare_values`
//! (engine `resolve.rs`) arm-for-arm. The function is the reusable comparison
//! helper for Stage-3 consumers migrating off `resolve_field` + `compare_values`.

use std::cmp::Ordering;

use crate::record_view::scalar_ref::scalar_ref_cmp;
use crate::record_view::ScalarRef;
use crate::types::value::InnerValue;

// ── Int / Int ───────────────────────────────────────────────────────────────

#[test]
fn int_int_equal() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Int(42), &InnerValue::Int(42)),
        Some(Ordering::Equal),
    );
}

#[test]
fn int_int_less() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Int(1), &InnerValue::Int(99)),
        Some(Ordering::Less),
    );
}

#[test]
fn int_int_greater() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Int(99), &InnerValue::Int(1)),
        Some(Ordering::Greater),
    );
}

// ── Cross-type Int / F64 (the semantic trap) ────────────────────────────────

#[test]
fn int_f64_equal_cross_type() {
    // The key invariant: Int(5) vs F64(5.0) must compare Equal.
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Int(5), &InnerValue::F64(5.0)),
        Some(Ordering::Equal),
    );
}

#[test]
fn f64_int_equal_cross_type() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::F64(5.0), &InnerValue::Int(5)),
        Some(Ordering::Equal),
    );
}

#[test]
fn int_f64_less() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Int(3), &InnerValue::F64(3.5)),
        Some(Ordering::Less),
    );
}

#[test]
fn f64_int_greater() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::F64(3.5), &InnerValue::Int(3)),
        Some(Ordering::Greater),
    );
}

// ── F64 / F64 ───────────────────────────────────────────────────────────────

#[test]
fn f64_f64_equal() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::F64(1.23), &InnerValue::F64(1.23)),
        Some(Ordering::Equal),
    );
}

#[test]
fn f64_f64_less() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::F64(1.0), &InnerValue::F64(2.0)),
        Some(Ordering::Less),
    );
}

#[test]
fn f64_nan_returns_none() {
    // NaN is not ordered — partial_cmp returns None, same as compare_values.
    assert_eq!(
        scalar_ref_cmp(ScalarRef::F64(f64::NAN), &InnerValue::F64(1.0)),
        None,
    );
}

// ── Str / Str ───────────────────────────────────────────────────────────────

#[test]
fn str_str_equal() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Str("hello"), &InnerValue::Str("hello".into())),
        Some(Ordering::Equal),
    );
}

#[test]
fn str_str_less() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Str("aaa"), &InnerValue::Str("bbb".into())),
        Some(Ordering::Less),
    );
}

#[test]
fn str_str_greater() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Str("z"), &InnerValue::Str("a".into())),
        Some(Ordering::Greater),
    );
}

// ── Bool ────────────────────────────────────────────────────────────────────

#[test]
fn bool_equal() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Bool(true), &InnerValue::Bool(true)),
        Some(Ordering::Equal),
    );
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Bool(false), &InnerValue::Bool(false)),
        Some(Ordering::Equal),
    );
}

#[test]
fn bool_less() {
    // false < true (bool::cmp)
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Bool(false), &InnerValue::Bool(true)),
        Some(Ordering::Less),
    );
}

// ── Null ────────────────────────────────────────────────────────────────────

#[test]
fn null_null_equal() {
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Null, &InnerValue::Null),
        Some(Ordering::Equal),
    );
}

// ── Cross-type Dec/Big comparison (no longer None) ─────────────────────────

#[test]
fn dec_cross_type_int() {
    // Int↔Dec is exact (Decimal represents every i64 exactly).
    assert_eq!(
        scalar_ref_cmp(
            ScalarRef::Int(1),
            &InnerValue::Dec(rust_decimal::Decimal::new(1, 0))
        ),
        Some(Ordering::Equal),
    );
    assert_eq!(
        scalar_ref_cmp(
            ScalarRef::Int(5),
            &InnerValue::Dec(rust_decimal::Decimal::new(10, 0))
        ),
        Some(Ordering::Less),
    );
}

#[test]
fn dec_cross_type_f64() {
    // F64↔Dec uses the f64 fallback.
    assert_eq!(
        scalar_ref_cmp(
            ScalarRef::F64(5.0),
            &InnerValue::Dec(rust_decimal::Decimal::new(10, 0))
        ),
        Some(Ordering::Less),
    );
}

#[test]
fn container_returns_none() {
    use crate::types::common::new_map_wc;
    // ScalarRef::Str vs InnerValue::Map → None.
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Str("x"), &InnerValue::Map(new_map_wc(0))),
        None,
    );
    // ScalarRef::Int vs InnerValue::List → None.
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Int(1), &InnerValue::List(vec![])),
        None,
    );
}

#[test]
fn mismatched_type_families_none() {
    // Int vs Str → None (same as compare_values).
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Int(1), &InnerValue::Str("1".into())),
        None,
    );
    // Str vs Bool → None.
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Str("true"), &InnerValue::Bool(true)),
        None,
    );
    // Null vs Int → None.
    assert_eq!(scalar_ref_cmp(ScalarRef::Null, &InnerValue::Int(0)), None,);
}

#[test]
fn bin_vs_anything_returns_none() {
    // Bin is a ScalarRef variant but compare_values has no Bin arm → None.
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Bin(&[1, 2, 3]), &InnerValue::Bin(vec![1, 2, 3])),
        None,
    );
}
