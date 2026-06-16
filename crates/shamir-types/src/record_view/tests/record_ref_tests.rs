//! Substitutability parity tests for the `RecordRef` trait — the keystone of
//! Stage 2. For a battery of paths and scalar types, asserts that:
//!
//!   `InnerValue.method(path)  ==  RecordView::new(&bytes).method(path)`
//!
//! This is the contract that makes the Stage-4 cutover safe: both impls return
//! identical results for the same interned-id path.

use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{Kind, RecordRef, RecordView, ScalarRef};
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

/// Assert parity for `present_kind_at`.
fn assert_kind_parity(iv: &InnerValue, view: &RecordView<'_>, path: &[InternerKey], label: &str) {
    let from_tree = iv.present_kind_at(path);
    let from_lens = view.present_kind_at(path);
    // Known divergence: InnerValue::Dec/Big map to NonComparable but the lens
    // sees the serialised Str marker and returns Scalar. For Dec/Big fields
    // we assert BOTH sides independently (not equal). For everything else:
    // parity.
    if from_tree == Some(Kind::NonComparable) {
        // The lens cannot distinguish Dec/Big from Str — it returns Scalar.
        assert_eq!(
            from_lens,
            Some(Kind::Scalar),
            "kind parity (Dec/Big) for '{label}': tree={from_tree:?}, lens={from_lens:?}"
        );
    } else {
        assert_eq!(
            from_tree, from_lens,
            "kind parity failure for '{label}': tree={from_tree:?}, lens={from_lens:?}"
        );
    }
}

/// Assert parity for `exists_at`.
fn assert_exists_parity(iv: &InnerValue, view: &RecordView<'_>, path: &[InternerKey], label: &str) {
    let from_tree = iv.exists_at(path);
    let from_lens = view.exists_at(path);
    assert_eq!(
        from_tree, from_lens,
        "exists_at parity failure for '{label}': tree={from_tree}, lens={from_lens}"
    );
}

/// Assert parity for `is_null_at`.
fn assert_is_null_parity(
    iv: &InnerValue,
    view: &RecordView<'_>,
    path: &[InternerKey],
    label: &str,
) {
    let from_tree = iv.is_null_at(path);
    let from_lens = view.is_null_at(path);
    assert_eq!(
        from_tree, from_lens,
        "is_null_at parity failure for '{label}': tree={from_tree}, lens={from_lens}"
    );
}

/// Assert parity for `str_at`.
fn assert_str_at_parity(iv: &InnerValue, view: &RecordView<'_>, path: &[InternerKey], label: &str) {
    let from_tree = iv.str_at(path);
    let from_lens = view.str_at(path);
    assert_eq!(
        from_tree, from_lens,
        "str_at parity failure for '{label}': tree={from_tree:?}, lens={from_lens:?}"
    );
}

/// Assert parity for `materialize_at`. Both sides must return the same
/// `InnerValue` subtree. We compare the round-tripped tree (from_bytes of
/// to_bytes) because Dec/Big undergo a Str collapse in serialisation.
fn assert_materialize_parity(
    iv: &InnerValue,
    view: &RecordView<'_>,
    bytes: &[u8],
    path: &[InternerKey],
    label: &str,
) {
    let from_lens = view.materialize_at(path);
    // For the tree side, materialize_at clones the subtree. But the original
    // tree may have Dec/Big that the lens sees as Str. Use the round-tripped
    // tree for comparison.
    let rt_iv = InnerValue::from_bytes(bytes).unwrap();
    let from_tree_rt = rt_iv.materialize_at(path);
    assert_eq!(
        from_tree_rt, from_lens,
        "materialize_at parity failure for '{label}': tree_rt={from_tree_rt:?}, lens={from_lens:?}"
    );
    // Also verify that the original tree's materialize_at works for non-Dec/Big.
    let from_tree_orig = iv.materialize_at(path);
    if from_tree_orig != from_tree_rt {
        // Dec/Big divergence — expected. Verify the lens matches the rt tree.
        assert_eq!(
            from_tree_rt, from_lens,
            "materialize_at rt parity for '{label}'"
        );
    } else {
        assert_eq!(
            from_tree_orig, from_lens,
            "materialize_at orig parity for '{label}'"
        );
    }
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
    let k_list_mixed = ik(&interner, "list_mixed");
    let k_list_empty = ik(&interner, "list_empty");

    // Build the nested sub-map: { inner_field: 42, deep: { leaf: "hello" } }
    let mut deep_map = new_map_wc(1);
    deep_map.insert(k_leaf.clone(), InnerValue::Str("hello".to_owned()));

    let mut nested_map = new_map_wc(2);
    nested_map.insert(k_inner.clone(), InnerValue::Int(42));
    nested_map.insert(k_deep, InnerValue::Map(deep_map));

    // Build a sub-map for "container leaf" test
    let mut sub_map = new_map_wc(1);
    sub_map.insert(k_inner, InnerValue::Int(99));

    // A list with mixed scalar + container elements (for any_seq_elem skip test)
    let mut nested_sub = new_map_wc(1);
    nested_sub.insert(k_leaf, InnerValue::Int(7));
    let list_mixed = vec![
        InnerValue::Int(10),
        InnerValue::Str("abc".to_owned()),
        InnerValue::Map(nested_sub), // container — should be skipped
        InnerValue::Int(20),
    ];

    // Root map
    let mut root = new_map_wc(20);
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
    root.insert(k_list_mixed, InnerValue::List(list_mixed));
    root.insert(k_list_empty, InnerValue::List(Vec::new()));

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

// ============================================================================
// present_kind_at parity
// ============================================================================

#[test]
fn kind_parity_battery() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    // (path, label, expected_tree_kind)
    let cases: Vec<(Vec<InternerKey>, &str, Option<Kind>)> = vec![
        (vec![ik(&int, "null_field")], "null", Some(Kind::Null)),
        (vec![ik(&int, "bool_true")], "bool_t", Some(Kind::Scalar)),
        (vec![ik(&int, "int_pos")], "int+", Some(Kind::Scalar)),
        (vec![ik(&int, "f64_val")], "f64", Some(Kind::Scalar)),
        (vec![ik(&int, "str_val")], "str", Some(Kind::Scalar)),
        (vec![ik(&int, "bin_val")], "bin", Some(Kind::Scalar)),
        (
            vec![ik(&int, "dec_field")],
            "dec",
            Some(Kind::NonComparable),
        ),
        (vec![ik(&int, "arr_field")], "arr", Some(Kind::Container)),
        (vec![ik(&int, "map_leaf")], "map", Some(Kind::Container)),
        (
            vec![ik(&int, "nested")],
            "nested_map",
            Some(Kind::Container),
        ),
        (
            vec![ik(&int, "nested"), ik(&int, "inner_field")],
            "nested.inner",
            Some(Kind::Scalar),
        ),
        (
            vec![ik(&int, "nested"), ik(&int, "deep")],
            "nested.deep",
            Some(Kind::Container),
        ),
        (vec![ik(&int, "no_such_field")], "missing", None),
        (
            vec![ik(&int, "int_pos"), ik(&int, "inner_field")],
            "thru_int",
            None,
        ),
        (vec![], "empty", None),
    ];

    for (path, label, expected_tree) in &cases {
        let from_tree = iv.present_kind_at(path);
        assert_eq!(
            &from_tree, expected_tree,
            "tree kind for '{label}': got {from_tree:?}, expected {expected_tree:?}"
        );
        assert_kind_parity(&iv, &view, path, label);
    }
}

// ============================================================================
// str_at parity
// ============================================================================

#[test]
fn str_at_parity_battery() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    // (path, label, expected)
    let cases: Vec<(Vec<InternerKey>, &str, Option<&str>)> = vec![
        (vec![ik(&int, "str_val")], "str", Some("shamir")),
        (vec![ik(&int, "str_empty")], "str_empty", Some("")),
        (vec![ik(&int, "int_pos")], "int_not_str", None),
        (vec![ik(&int, "null_field")], "null_not_str", None),
        (vec![ik(&int, "bool_true")], "bool_not_str", None),
        (vec![ik(&int, "f64_val")], "f64_not_str", None),
        (vec![ik(&int, "bin_val")], "bin_not_str", None),
        (vec![ik(&int, "arr_field")], "arr_not_str", None),
        (vec![ik(&int, "map_leaf")], "map_not_str", None),
        (vec![ik(&int, "no_such_field")], "missing", None),
        (
            vec![ik(&int, "nested"), ik(&int, "deep"), ik(&int, "leaf")],
            "nested_str",
            Some("hello"),
        ),
        (vec![], "empty_path", None),
    ];

    for (path, label, expected) in &cases {
        let from_tree = iv.str_at(path);
        assert_eq!(
            &from_tree, expected,
            "tree str_at for '{label}': got {from_tree:?}, expected {expected:?}"
        );
        assert_str_at_parity(&iv, &view, path, label);
    }
}

// ============================================================================
// exists_at / is_null_at parity
// ============================================================================

#[test]
fn exists_at_parity_battery() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    let cases: Vec<(Vec<InternerKey>, &str, bool)> = vec![
        (vec![ik(&int, "null_field")], "null_exists", true),
        (vec![ik(&int, "int_pos")], "int_exists", true),
        (vec![ik(&int, "str_val")], "str_exists", true),
        (vec![ik(&int, "arr_field")], "arr_exists", true),
        (vec![ik(&int, "map_leaf")], "map_exists", true),
        (vec![ik(&int, "dec_field")], "dec_exists", true),
        (
            vec![ik(&int, "nested"), ik(&int, "inner_field")],
            "nested_exists",
            true,
        ),
        (vec![ik(&int, "no_such_field")], "missing_not_exists", false),
        (
            vec![ik(&int, "int_pos"), ik(&int, "inner_field")],
            "thru_scalar",
            false,
        ),
        (vec![], "empty_path", false),
    ];

    for (path, label, expected) in &cases {
        let from_tree = iv.exists_at(path);
        assert_eq!(
            from_tree, *expected,
            "tree exists_at for '{label}': got {from_tree}, expected {expected}"
        );
        assert_exists_parity(&iv, &view, path, label);
    }
}

#[test]
fn is_null_at_parity_battery() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    let cases: Vec<(Vec<InternerKey>, &str, bool)> = vec![
        (vec![ik(&int, "null_field")], "null_is_null", true),
        (vec![ik(&int, "int_pos")], "int_not_null", false),
        (vec![ik(&int, "str_val")], "str_not_null", false),
        (vec![ik(&int, "arr_field")], "arr_not_null", false),
        (vec![ik(&int, "no_such_field")], "missing_is_null", true),
        (vec![], "empty_path_is_null", true),
        (
            vec![ik(&int, "nested"), ik(&int, "inner_field")],
            "nested_not_null",
            false,
        ),
    ];

    for (path, label, expected) in &cases {
        let from_tree = iv.is_null_at(path);
        assert_eq!(
            from_tree, *expected,
            "tree is_null_at for '{label}': got {from_tree}, expected {expected}"
        );
        assert_is_null_parity(&iv, &view, path, label);
    }
}

// ============================================================================
// any_seq_elem parity
// ============================================================================

/// Generic helper: runs `any_seq_elem` on both impls with the same predicate.
fn assert_any_seq_parity(
    iv: &InnerValue,
    view: &RecordView<'_>,
    path: &[InternerKey],
    target: ScalarRef<'_>,
    label: &str,
) {
    let from_tree = iv.any_seq_elem(path, &mut |sr| sr == target);
    let from_lens = view.any_seq_elem(path, &mut |sr| sr == target);
    assert_eq!(
        from_tree, from_lens,
        "any_seq_elem parity failure for '{label}' target={target:?}: tree={from_tree:?}, lens={from_lens:?}"
    );
}

#[test]
fn any_seq_elem_list_match() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let path = [ik(&int, "arr_field")];

    // arr_field = [1, 2] — looking for 1 should return Some(true)
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(1), "arr_match_1");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(1)),
        Some(true)
    );
}

#[test]
fn any_seq_elem_list_no_match() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let path = [ik(&int, "arr_field")];

    // arr_field = [1, 2] — looking for 99 should return Some(false)
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(99), "arr_no_match");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(99)),
        Some(false)
    );
}

#[test]
fn any_seq_elem_empty_list() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let path = [ik(&int, "list_empty")];

    // Empty list — any search returns Some(false)
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(1), "empty_list");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(1)),
        Some(false)
    );
}

#[test]
fn any_seq_elem_non_list_returns_none() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    // Scalar path — not a list/set
    let path = [ik(&int, "int_pos")];
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(12345), "non_list_int");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(12345)),
        None
    );

    // Map path — not a list/set
    let path = [ik(&int, "map_leaf")];
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(99), "non_list_map");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(99)),
        None
    );

    // Missing path
    let path = [ik(&int, "no_such_field")];
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(1), "non_list_missing");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(1)),
        None
    );
}

#[test]
fn any_seq_elem_mixed_list_skips_containers() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let path = [ik(&int, "list_mixed")];

    // list_mixed = [10, "abc", {leaf: 7}, 20]
    // Match scalar 10 — should find it
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(10), "mixed_match_10");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(10)),
        Some(true)
    );

    // Match "abc"
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Str("abc"), "mixed_match_abc");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Str("abc")),
        Some(true)
    );

    // Match 20 (after the container element)
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(20), "mixed_match_20");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(20)),
        Some(true)
    );

    // No match for 99 — the container {leaf: 7} is skipped
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(99), "mixed_no_match");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(99)),
        Some(false)
    );
}

// ============================================================================
// materialize_at parity
// ============================================================================

#[test]
fn materialize_at_scalar() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    // Scalar int
    assert_materialize_parity(&iv, &view, &bytes, &[ik(&int, "int_pos")], "mat_int");
    assert_eq!(
        iv.materialize_at(&[ik(&int, "int_pos")]),
        Some(InnerValue::Int(12345))
    );

    // Scalar str
    assert_materialize_parity(&iv, &view, &bytes, &[ik(&int, "str_val")], "mat_str");
    assert_eq!(
        iv.materialize_at(&[ik(&int, "str_val")]),
        Some(InnerValue::Str("shamir".to_owned()))
    );

    // Null
    assert_materialize_parity(&iv, &view, &bytes, &[ik(&int, "null_field")], "mat_null");
    assert_eq!(
        iv.materialize_at(&[ik(&int, "null_field")]),
        Some(InnerValue::Null)
    );

    // Bool
    assert_materialize_parity(&iv, &view, &bytes, &[ik(&int, "bool_true")], "mat_bool");
}

#[test]
fn materialize_at_nested_map() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    // Nested map subtree
    let path = [ik(&int, "nested"), ik(&int, "deep")];
    assert_materialize_parity(&iv, &view, &bytes, &path, "mat_nested_map");

    // The materialised value should be a Map containing {leaf: "hello"}
    let mat = view.materialize_at(&path).unwrap();
    if let InnerValue::Map(m) = &mat {
        assert_eq!(
            m.get(&ik(&int, "leaf")),
            Some(&InnerValue::Str("hello".to_owned()))
        );
    } else {
        panic!("expected Map, got {mat:?}");
    }
}

#[test]
fn materialize_at_array() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    let path = [ik(&int, "arr_field")];
    assert_materialize_parity(&iv, &view, &bytes, &path, "mat_array");

    let mat = view.materialize_at(&path).unwrap();
    assert_eq!(
        mat,
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)])
    );
}

#[test]
fn materialize_at_missing() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    let path = [ik(&int, "no_such_field")];
    assert_eq!(iv.materialize_at(&path), None);
    assert_eq!(view.materialize_at(&path), None);
}

#[test]
fn materialize_at_empty_path() {
    let (iv, _int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    assert_eq!(iv.materialize_at(&[]), None);
    assert_eq!(view.materialize_at(&[]), None);
}

#[test]
fn materialize_at_nested_scalar() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    let path = [ik(&int, "nested"), ik(&int, "inner_field")];
    assert_materialize_parity(&iv, &view, &bytes, &path, "mat_nested_scalar");
    assert_eq!(view.materialize_at(&path), Some(InnerValue::Int(42)));
}

// ============================================================================
// for_each_field parity
// ============================================================================

#[test]
fn for_each_field_parity() {
    let (iv, _int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let rt_iv = InnerValue::from_bytes(&bytes).unwrap();
    let view = RecordView::new(&bytes).unwrap();

    // Collect from round-tripped tree.
    let mut tree_fields = Vec::new();
    rt_iv.for_each_field(&mut |k, v| tree_fields.push((k, v)));

    // Collect from lens.
    let mut lens_fields = Vec::new();
    view.for_each_field(&mut |k, v| lens_fields.push((k, v)));

    // Both should have the same number of fields.
    assert_eq!(
        tree_fields.len(),
        lens_fields.len(),
        "field count mismatch: tree={}, lens={}",
        tree_fields.len(),
        lens_fields.len()
    );

    // Convert both to sorted-by-key-id sets for order-independent comparison.
    tree_fields.sort_by_key(|(k, _)| k.id());
    lens_fields.sort_by_key(|(k, _)| k.id());

    for ((tk, tv), (lk, lv)) in tree_fields.iter().zip(lens_fields.iter()) {
        assert_eq!(tk, lk, "key mismatch: tree={tk:?}, lens={lk:?}");
        assert_eq!(
            tv, lv,
            "value mismatch for key {tk:?}: tree={tv:?}, lens={lv:?}"
        );
    }
}

#[test]
fn for_each_field_empty_map() {
    let iv = InnerValue::Map(new_map_wc(0));
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    let mut tree_count = 0;
    iv.for_each_field(&mut |_, _| tree_count += 1);

    let mut lens_count = 0;
    view.for_each_field(&mut |_, _| lens_count += 1);

    assert_eq!(tree_count, 0);
    assert_eq!(lens_count, 0);
}

#[test]
fn for_each_field_non_map_tree() {
    // Calling for_each_field on a non-map InnerValue should be a no-op.
    let iv = InnerValue::Int(42);
    let mut count = 0;
    iv.for_each_field(&mut |_, _| count += 1);
    assert_eq!(count, 0);
}
