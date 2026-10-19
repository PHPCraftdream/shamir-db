//! Scalar parity tests: one scalar kind at a time.

use super::helpers::{assert_parity, build_record, ik, probe};
use crate::record_view::{RecordRef, RecordView, ScalarRef};
use crate::types::value::InnerValue;

#[test]
fn parity_null() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    assert_parity(&iv, &bytes, &[ik(&int, "null_field")], "null");
}

#[test]
fn parity_bool_true() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let result = probe(&iv, &[ik(&int, "bool_true")]);
    assert_eq!(result, Some(ScalarRef::Bool(true)));
    assert_parity(&iv, &bytes, &[ik(&int, "bool_true")], "bool_true");
}

#[test]
fn parity_bool_false() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    assert_parity(&iv, &bytes, &[ik(&int, "bool_false")], "bool_false");
}

#[test]
fn parity_int_positive() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let result = probe(&iv, &[ik(&int, "int_pos")]);
    assert_eq!(result, Some(ScalarRef::Int(12345)));
    assert_parity(&iv, &bytes, &[ik(&int, "int_pos")], "int_pos");
}

#[test]
fn parity_int_negative() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    assert_parity(&iv, &bytes, &[ik(&int, "int_neg")], "int_neg");
}

#[test]
fn parity_int_zero() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    assert_parity(&iv, &bytes, &[ik(&int, "int_zero")], "int_zero");
}

#[test]
fn parity_f64() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let result = probe(&iv, &[ik(&int, "f64_val")]);
    assert_eq!(result, Some(ScalarRef::F64(1.23)));
    assert_parity(&iv, &bytes, &[ik(&int, "f64_val")], "f64");
}

#[test]
fn parity_f64_negative_zero() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    // -0.0 and 0.0 have different bits; ScalarRef::PartialEq uses to_bits().
    assert_parity(&iv, &bytes, &[ik(&int, "f64_neg")], "f64_neg_zero");
}

#[test]
fn parity_str() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let result = probe(&iv, &[ik(&int, "str_val")]);
    assert_eq!(result, Some(ScalarRef::Str("shamir")));
    assert_parity(&iv, &bytes, &[ik(&int, "str_val")], "str");
}

#[test]
fn parity_str_empty() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    assert_parity(&iv, &bytes, &[ik(&int, "str_empty")], "str_empty");
}

#[test]
fn parity_bin() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let result = probe(&iv, &[ik(&int, "bin_val")]);
    assert_eq!(result, Some(ScalarRef::Bin(&[0xDE, 0xAD, 0xBE, 0xEF])));
    assert_parity(&iv, &bytes, &[ik(&int, "bin_val")], "bin");
}

#[test]
fn parity_bin_empty() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    assert_parity(&iv, &bytes, &[ik(&int, "bin_empty")], "bin_empty");
}

// ── Parity: nested path (multi-segment) ──────────────────────────────────────

#[test]
fn parity_nested_scalar() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let path = [ik(&int, "nested"), ik(&int, "inner_field")];
    let result = probe(&iv, &path);
    assert_eq!(result, Some(ScalarRef::Int(42)));
    assert_parity(&iv, &bytes, &path, "nested.inner_field");
}

#[test]
fn parity_deep_nested_scalar() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let path = [ik(&int, "nested"), ik(&int, "deep"), ik(&int, "leaf")];
    let result = probe(&iv, &path);
    assert_eq!(result, Some(ScalarRef::Str("hello")));
    assert_parity(&iv, &bytes, &path, "nested.deep.leaf");
}

// ── Parity: missing field ────────────────────────────────────────────────────

#[test]
fn parity_missing_field() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let missing = ik(&int, "no_such_field");
    assert_parity(&iv, &bytes, std::slice::from_ref(&missing), "missing_field");
    // Confirm it's actually None.
    let result = probe(&iv, &[missing]);
    assert_eq!(result, None);
}

#[test]
fn parity_missing_nested() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let path = [ik(&int, "nested"), ik(&int, "no_such_field")];
    assert_parity(&iv, &bytes, &path, "nested.missing");
}

// ── Parity: path through non-map ────────────────────────────────────────────

#[test]
fn parity_path_through_scalar() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    // Try to descend through an integer field — must return None.
    let path = [ik(&int, "int_pos"), ik(&int, "inner_field")];
    assert_parity(&iv, &bytes, &path, "through_int");
    assert_eq!(probe(&iv, &path), None);
}

#[test]
fn parity_path_through_array() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    // Try to descend through an array — must return None.
    let path = [ik(&int, "arr_field"), ik(&int, "inner_field")];
    assert_parity(&iv, &bytes, &path, "through_array");
    assert_eq!(probe(&iv, &path), None);
}

#[test]
fn parity_path_through_str() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let path = [ik(&int, "str_val"), ik(&int, "inner_field")];
    assert_parity(&iv, &bytes, &path, "through_str");
    assert_eq!(probe(&iv, &path), None);
}

// ── Parity: container leaf → None ────────────────────────────────────────────

#[test]
fn parity_container_leaf_map() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    // The leaf is a Map — scalar_at must return None.
    let path = [ik(&int, "map_leaf")];
    assert_parity(&iv, &bytes, &path, "container_map");
    assert_eq!(probe(&iv, &path), None);
}

#[test]
fn parity_container_leaf_array() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    // The leaf is a List/Array — scalar_at must return None.
    let path = [ik(&int, "arr_field")];
    assert_parity(&iv, &bytes, &path, "container_array");
    assert_eq!(probe(&iv, &path), None);
}

#[test]
fn parity_container_leaf_nested_map() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    // nested.deep is a Map — scalar_at must return None.
    let path = [ik(&int, "nested"), ik(&int, "deep")];
    assert_parity(&iv, &bytes, &path, "container_nested_map");
    assert_eq!(probe(&iv, &path), None);
}

// ── Parity: Dec leaf → None (non-comparable) ────────────────────────────────

#[test]
fn parity_dec_leaf() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    // Dec is serialised as Str by the encoder, but InnerValue::Dec is a
    // non-comparable type in compare_values. However, after round-trip through
    // msgpack, the tree decodes it as Str (since Dec is serialised via
    // serialize_str). So from the tree's InnerValue, the Dec field becomes
    // InnerValue::Str after to_bytes/from_bytes round-trip. The original
    // InnerValue::Dec maps to None in our impl. But RecordView sees the
    // bytes as a string. We test the ORIGINAL InnerValue here (before
    // round-trip), so tree sees Dec -> None.
    //
    // For the RecordView (bytes path), the Dec was serialised as Str, so
    // the lens sees Str -> ScalarRef::Str. This is a known DIVERGENCE
    // between the original InnerValue and the bytes representation. The
    // parity contract is: "same bytes, same result". When both go through
    // bytes, they agree. We test this by round-tripping.
    let rt_iv = InnerValue::from_bytes(&bytes).unwrap();
    let from_tree = probe(&rt_iv, &[ik(&int, "dec_field")]);
    let view = RecordView::new(&bytes).unwrap();
    let from_lens = probe(&view, &[ik(&int, "dec_field")]);
    assert_eq!(
        from_tree, from_lens,
        "parity after round-trip for dec_field"
    );
}

// ── Edge: empty path → None ─────────────────────────────────────────────────

#[test]
fn parity_empty_path() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let _int = int; // suppress unused
    assert_parity(&iv, &bytes, &[], "empty_path");
    assert_eq!(probe(&iv, &[]), None);
}

// ── scalar() convenience ────────────────────────────────────────────────────

#[test]
fn scalar_convenience_method() {
    let (iv, int) = build_record();
    let k = ik(&int, "int_pos");
    let result = iv.scalar(k);
    assert_eq!(result, Some(ScalarRef::Int(12345)));
}

// ── Comprehensive battery: all types via a single loop ──────────────────────
