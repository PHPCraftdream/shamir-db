//! `for_each_field` parity tests.

use super::helpers::build_record;
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
