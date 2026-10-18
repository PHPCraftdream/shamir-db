//! Shared helpers for the [`RecordRef`] parity tests. Logic helpers only —
//! re-exports belong in `mod.rs`.

use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{Kind, RecordRef, RecordView, ScalarRef};
use crate::types::common::new_map_wc;
use crate::types::value::InnerValue;

/// Intern a string, returning the `InternerKey`.
pub(super) fn ik(interner: &Interner, s: &str) -> InternerKey {
    interner.touch_ind(s).unwrap().into_key()
}

/// The generic probe — calls `scalar_at` through the trait. Static dispatch.
pub(super) fn probe<'a>(r: &'a impl RecordRef, path: &[InternerKey]) -> Option<ScalarRef<'a>> {
    r.scalar_at(path)
}

/// Assert parity: `InnerValue` and `RecordView` must return identical
/// `ScalarRef` for the same path.
pub(super) fn assert_parity(iv: &InnerValue, bytes: &[u8], path: &[InternerKey], label: &str) {
    let view = RecordView::new(bytes).expect("valid msgpack");
    let from_tree = probe(iv, path);
    let from_lens = probe(&view, path);
    assert_eq!(
        from_tree, from_lens,
        "parity failure for path '{label}': tree={from_tree:?}, lens={from_lens:?}"
    );
}

/// Assert parity for `present_kind_at`.
pub(super) fn assert_kind_parity(
    iv: &InnerValue,
    view: &RecordView<'_>,
    path: &[InternerKey],
    label: &str,
) {
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
pub(super) fn assert_exists_parity(
    iv: &InnerValue,
    view: &RecordView<'_>,
    path: &[InternerKey],
    label: &str,
) {
    let from_tree = iv.exists_at(path);
    let from_lens = view.exists_at(path);
    assert_eq!(
        from_tree, from_lens,
        "exists_at parity failure for '{label}': tree={from_tree}, lens={from_lens}"
    );
}

/// Assert parity for `is_null_at`.
pub(super) fn assert_is_null_parity(
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
pub(super) fn assert_str_at_parity(
    iv: &InnerValue,
    view: &RecordView<'_>,
    path: &[InternerKey],
    label: &str,
) {
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
pub(super) fn assert_materialize_parity(
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
pub(super) fn build_record() -> (InnerValue, Interner) {
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
