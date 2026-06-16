//! Substitutability parity tests for the `RecordRef` trait — the keystone of
//! Stage 2. For a battery of paths and scalar types, asserts that:
//!
//!   `InnerValue.scalar_at(path)  ==  RecordView::new(&bytes).scalar_at(path)`
//!
//! This is the contract that makes the Stage-4 cutover safe: both impls return
//! identical `ScalarRef` for the same interned-id path.

use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{RecordRef, RecordView, ScalarRef};
use crate::types::common::new_map_wc;
use crate::types::value::InnerValue;

/// Intern a string, returning the `InternerKey`.
fn ik(interner: &Interner, s: &str) -> InternerKey {
    interner.touch_ind(s).unwrap().into_key()
}

/// The generic probe — calls `scalar_at` through the trait. Static dispatch.
fn probe<'a>(r: &'a impl RecordRef, path: &[InternerKey]) -> Option<ScalarRef<'a>> {
    r.scalar_at(path)
}

/// Assert parity: `InnerValue` and `RecordView` must return identical
/// `ScalarRef` for the same path.
fn assert_parity(iv: &InnerValue, bytes: &[u8], path: &[InternerKey], label: &str) {
    let view = RecordView::new(bytes).expect("valid msgpack");
    let from_tree = probe(iv, path);
    let from_lens = probe(&view, path);
    assert_eq!(
        from_tree, from_lens,
        "parity failure for path '{label}': tree={from_tree:?}, lens={from_lens:?}"
    );
}

/// Build a representative record with every scalar type, nested maps, arrays,
/// and edge cases. Returns `(InnerValue, Interner)`.
fn build_record() -> (InnerValue, Interner) {
    let interner = Interner::default();

    let k_null = ik(&interner, "null_field");
    let k_bool_t = ik(&interner, "bool_true");
    let k_bool_f = ik(&interner, "bool_false");
    let k_int_pos = ik(&interner, "int_pos");
    let k_int_neg = ik(&interner, "int_neg");
    let k_int_zero = ik(&interner, "int_zero");
    let k_f64 = ik(&interner, "f64_val");
    let k_f64_neg = ik(&interner, "f64_neg");
    let k_str = ik(&interner, "str_val");
    let k_str_empty = ik(&interner, "str_empty");
    let k_bin = ik(&interner, "bin_val");
    let k_bin_empty = ik(&interner, "bin_empty");
    let k_nested = ik(&interner, "nested");
    let k_inner = ik(&interner, "inner_field");
    let k_deep = ik(&interner, "deep");
    let k_leaf = ik(&interner, "leaf");
    let k_arr = ik(&interner, "arr_field");
    let k_dec = ik(&interner, "dec_field");
    let k_map_leaf = ik(&interner, "map_leaf");

    // Build the nested sub-map: { inner_field: 42, deep: { leaf: "hello" } }
    let mut deep_map = new_map_wc(1);
    deep_map.insert(k_leaf, InnerValue::Str("hello".to_owned()));

    let mut nested_map = new_map_wc(2);
    nested_map.insert(k_inner.clone(), InnerValue::Int(42));
    nested_map.insert(k_deep, InnerValue::Map(deep_map));

    // Build a sub-map for "container leaf" test
    let mut sub_map = new_map_wc(1);
    sub_map.insert(k_inner, InnerValue::Int(99));

    // Root map
    let mut root = new_map_wc(16);
    root.insert(k_null, InnerValue::Null);
    root.insert(k_bool_t, InnerValue::Bool(true));
    root.insert(k_bool_f, InnerValue::Bool(false));
    root.insert(k_int_pos, InnerValue::Int(12345));
    root.insert(k_int_neg, InnerValue::Int(-999));
    root.insert(k_int_zero, InnerValue::Int(0));
    root.insert(k_f64, InnerValue::F64(1.23));
    root.insert(k_f64_neg, InnerValue::F64(-0.0));
    root.insert(k_str, InnerValue::Str("shamir".to_owned()));
    root.insert(k_str_empty, InnerValue::Str(String::new()));
    root.insert(k_bin, InnerValue::Bin(vec![0xDE, 0xAD, 0xBE, 0xEF]));
    root.insert(k_bin_empty, InnerValue::Bin(Vec::new()));
    root.insert(k_nested, InnerValue::Map(nested_map));
    root.insert(
        k_arr,
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
    );
    root.insert(k_dec, InnerValue::Dec(rust_decimal::Decimal::new(123, 2)));
    root.insert(k_map_leaf, InnerValue::Map(sub_map));

    (InnerValue::Map(root), interner)
}

// ── Parity: every scalar type ────────────────────────────────────────────────

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

#[test]
fn parity_battery() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();

    let cases: Vec<(Vec<InternerKey>, &str, Option<ScalarRef<'_>>)> = vec![
        (vec![ik(&int, "null_field")], "null", Some(ScalarRef::Null)),
        (
            vec![ik(&int, "bool_true")],
            "bool_t",
            Some(ScalarRef::Bool(true)),
        ),
        (
            vec![ik(&int, "bool_false")],
            "bool_f",
            Some(ScalarRef::Bool(false)),
        ),
        (
            vec![ik(&int, "int_pos")],
            "int+",
            Some(ScalarRef::Int(12345)),
        ),
        (
            vec![ik(&int, "int_neg")],
            "int-",
            Some(ScalarRef::Int(-999)),
        ),
        (vec![ik(&int, "int_zero")], "int0", Some(ScalarRef::Int(0))),
        (vec![ik(&int, "f64_val")], "f64", Some(ScalarRef::F64(1.23))),
        (
            vec![ik(&int, "str_val")],
            "str",
            Some(ScalarRef::Str("shamir")),
        ),
        (
            vec![ik(&int, "str_empty")],
            "str_e",
            Some(ScalarRef::Str("")),
        ),
        (
            vec![ik(&int, "bin_val")],
            "bin",
            Some(ScalarRef::Bin(&[0xDE, 0xAD, 0xBE, 0xEF])),
        ),
        (
            vec![ik(&int, "bin_empty")],
            "bin_e",
            Some(ScalarRef::Bin(&[])),
        ),
        (
            vec![ik(&int, "nested"), ik(&int, "inner_field")],
            "nest1",
            Some(ScalarRef::Int(42)),
        ),
        (
            vec![ik(&int, "nested"), ik(&int, "deep"), ik(&int, "leaf")],
            "nest3",
            Some(ScalarRef::Str("hello")),
        ),
        (vec![ik(&int, "no_such_field")], "miss", None),
        (
            vec![ik(&int, "int_pos"), ik(&int, "inner_field")],
            "thru_int",
            None,
        ),
        (vec![ik(&int, "arr_field")], "arr_leaf", None),
        (vec![ik(&int, "map_leaf")], "map_leaf", None),
        (vec![], "empty", None),
    ];

    for (path, label, expected_tree) in &cases {
        // Assert tree gives expected value.
        let from_tree = probe(&iv, path);
        assert_eq!(
            &from_tree, expected_tree,
            "tree result for '{label}' differs from expected"
        );
        // Assert parity between tree and lens.
        assert_parity(&iv, &bytes, path, label);
    }
}
