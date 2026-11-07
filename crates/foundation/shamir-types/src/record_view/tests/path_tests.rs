//! `get_path` — nested `a.b.c` access via interned ids. Hit, miss,
//! descend-through-non-map, and empty-path edge cases.
//! Uses the storage form (`InnerValue::to_bytes()`) with id-keyed maps.

use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{RecordValue, RecordView};
use crate::types::common::new_map_wc;
use crate::types::value::InnerValue;

fn ik(interner: &Interner, s: &str) -> InternerKey {
    interner.touch_ind(s).unwrap().into_key()
}

#[test]
fn path_hit_two_levels() {
    let interner = Interner::new();
    let mut addr = new_map_wc(1);
    let city_key = ik(&interner, "city");
    addr.insert(city_key.clone(), InnerValue::Str("NYC".into()));
    let mut m = new_map_wc(1);
    let address_key = ik(&interner, "address");
    m.insert(address_key.clone(), InnerValue::Map(addr));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    let v = lens.get_path(&[address_key, city_key]).unwrap();
    assert_eq!(v.as_str(), Some("NYC"));
}

#[test]
fn path_hit_three_levels() {
    let interner = Interner::new();
    let mut leaf = new_map_wc(1);
    let v_key = ik(&interner, "v");
    leaf.insert(v_key.clone(), InnerValue::Int(99));
    let mut mid = new_map_wc(1);
    let leaf_key = ik(&interner, "leaf");
    mid.insert(leaf_key.clone(), InnerValue::Map(leaf));
    let mut root = new_map_wc(1);
    let mid_key = ik(&interner, "mid");
    root.insert(mid_key.clone(), InnerValue::Map(mid));
    let blob = InnerValue::Map(root).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert_eq!(
        lens.get_path(&[mid_key, leaf_key, v_key])
            .and_then(|v| v.as_int()),
        Some(99)
    );
}

#[test]
fn path_miss_first_component() {
    let interner = Interner::new();
    let mut m = new_map_wc(1);
    let a_key = ik(&interner, "a");
    m.insert(a_key, InnerValue::Int(1));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    let missing_key = ik(&interner, "missing");
    let b_key = ik(&interner, "b");
    assert_eq!(lens.get_path(&[missing_key, b_key]), None);
}

#[test]
fn path_miss_deep_component() {
    let interner = Interner::new();
    let mut mid = new_map_wc(1);
    let present_key = ik(&interner, "present");
    mid.insert(present_key, InnerValue::Int(1));
    let mut root = new_map_wc(1);
    let mid_key = ik(&interner, "mid");
    root.insert(mid_key.clone(), InnerValue::Map(mid));
    let blob = InnerValue::Map(root).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    let absent_key = ik(&interner, "absent");
    assert_eq!(lens.get_path(&[mid_key, absent_key]), None);
}

#[test]
fn path_descend_through_non_map_returns_none() {
    let interner = Interner::new();
    // a is an Int, not a map — descending a.b must yield None, not panic.
    let mut m = new_map_wc(3);
    let a_key = ik(&interner, "a");
    let s_key = ik(&interner, "s");
    let arr_key = ik(&interner, "arr");
    let b_key = ik(&interner, "b");
    let zero_key = ik(&interner, "0");
    m.insert(a_key.clone(), InnerValue::Int(1));
    m.insert(s_key.clone(), InnerValue::Str("str".into()));
    m.insert(arr_key.clone(), InnerValue::List(vec![InnerValue::Int(1)]));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert_eq!(lens.get_path(&[a_key, b_key.clone()]), None);
    assert_eq!(lens.get_path(&[s_key, b_key]), None);
    assert_eq!(lens.get_path(&[arr_key, zero_key]), None);
}

#[test]
fn path_descend_through_map_value_into_non_map_yields_value() {
    // The final component may be a scalar — get_path returns it (RecordValue).
    let interner = Interner::new();
    let mut mid = new_map_wc(1);
    let n_key = ik(&interner, "n");
    mid.insert(n_key.clone(), InnerValue::Int(5));
    let mut root = new_map_wc(1);
    let mid_key = ik(&interner, "mid");
    root.insert(mid_key.clone(), InnerValue::Map(mid));
    let blob = InnerValue::Map(root).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    let v = lens.get_path(&[mid_key, n_key]).unwrap();
    assert!(matches!(v, RecordValue::Int(5)));
}

#[test]
fn path_empty_returns_none() {
    let interner = Interner::new();
    let mut m = new_map_wc(1);
    let a_key = ik(&interner, "a");
    m.insert(a_key, InnerValue::Int(1));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert_eq!(lens.get_path(&[]), None);
}

#[test]
fn path_single_component_equivalent_to_get() {
    let interner = Interner::new();
    let mut m = new_map_wc(1);
    let a_key = ik(&interner, "a");
    m.insert(a_key.clone(), InnerValue::Int(1));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert_eq!(
        lens.get_path(std::slice::from_ref(&a_key))
            .and_then(|v| v.as_int()),
        lens.get_int(a_key)
    );
}
