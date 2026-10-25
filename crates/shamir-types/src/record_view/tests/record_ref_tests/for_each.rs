//! `for_each_field` parity tests.

use super::helpers::{build_record, ik};
use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{RecordRef, RecordView};
use crate::types::common::new_map_wc;
use crate::types::value::InnerValue;

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

#[test]
fn rv_types_1c6c4f54_for_each_field_single_pass_parity() {
    // Behavioural parity lock for the single-pass for_each_field_bytes rewrite
    // (perf card: the O(fields²) -> O(fields) property itself is structural).
    // Record: Int, Str, nested Map (with an inner field), Bin, List.
    let interner = Interner::new();
    let k_int = ik(&interner, "f_int");
    let k_str = ik(&interner, "f_str");
    let k_map = ik(&interner, "f_map");
    let k_inner = ik(&interner, "f_inner");
    let k_bin = ik(&interner, "f_bin");
    let k_list = ik(&interner, "f_list");

    let mut inner = new_map_wc(1);
    inner.insert(k_inner.clone(), InnerValue::Int(7));

    let mut m = new_map_wc(5);
    m.insert(k_int.clone(), InnerValue::Int(-3));
    m.insert(k_str.clone(), InnerValue::Str("hello".to_owned()));
    m.insert(k_map.clone(), InnerValue::Map(inner));
    m.insert(k_bin.clone(), InnerValue::Bin(vec![0xDE, 0xAD]));
    m.insert(k_list.clone(), InnerValue::List(vec![InnerValue::Int(1)]));

    let bytes = InnerValue::Map(m).to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    // Insertion order of the record's top-level fields.
    let expected_order = [
        k_int.clone(),
        k_str.clone(),
        k_map.clone(),
        k_bin.clone(),
        k_list.clone(),
    ];

    let mut visited: Vec<(InternerKey, InnerValue)> = Vec::new();
    view.for_each_field(&mut |k, v| visited.push((k, v)));

    // (a) every field visited exactly once with the inserted value.
    assert_eq!(visited.len(), expected_order.len());
    // (b) order matches record insertion order.
    for ((k, v), ek) in visited.iter().zip(expected_order.iter()) {
        assert_eq!(k, ek, "field order mismatch");
        let mat = view.materialize_at(std::slice::from_ref(k)).unwrap();
        assert_eq!(*v, mat, "value mismatch for key {k:?}");
    }
}

/// Deterministic linear-work bound for `for_each_field` (RV-TYPES-1C6C4F54):
/// the single-pass byte-range walk costs 2 scan steps/field (one read_bin_len
/// + one skip_value). The old `fields()` + `materialize_at` loop rescans from
///   offset 0 per field (~n² steps) and blows the bound.
#[test]
fn rv_types_1c6c4f54_for_each_field_linear_scan() {
    let interner = Interner::new();
    let n = 60usize;
    let mut m = new_map_wc(n);
    let mut keys = Vec::with_capacity(n);
    for i in 0..n {
        let k = ik(&interner, &format!("g{i:02}"));
        m.insert(k.clone(), InnerValue::Int(i as i64));
        keys.push(k);
    }
    let bytes = InnerValue::Map(m).to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    crate::record_view::reset_scan_steps();
    let mut count = 0usize;
    view.for_each_field(&mut |k, _v| {
        assert!(keys.iter().any(|ek| ek == &k), "unexpected key {k:?}");
        count += 1;
    });
    let steps = crate::record_view::scan_steps();

    // Every field visited exactly once (semantics pinned).
    assert_eq!(count, n, "field visit count mismatch");
    // Linear bound: 2 steps/field on the single pass (+ slack).
    assert!(
        steps <= 3 * n as u64 + 20,
        "scan work {steps} exceeds linear bound {} — per-field rescan reverted?",
        3 * n as u64 + 20
    );
}
