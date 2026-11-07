//! `match_str_eq` — filter-eval on BYTES. True / false / missing / wrong-type.
//! Uses the storage form (`InnerValue::to_bytes()`) with id-keyed maps.

use crate::core::interner::{Interner, InternerKey};
use crate::record_view::RecordView;
use crate::types::common::new_map_wc;
use crate::types::value::InnerValue;

fn ik(interner: &Interner, s: &str) -> InternerKey {
    interner.touch_ind(s).unwrap().into_key()
}

#[test]
fn match_true() {
    let interner = Interner::new();
    let mut m = new_map_wc(1);
    let city_key = ik(&interner, "city");
    m.insert(city_key.clone(), InnerValue::Str("NYC".into()));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert!(lens.match_str_eq(city_key, b"NYC"));
}

#[test]
fn match_false_value_differs() {
    let interner = Interner::new();
    let mut m = new_map_wc(1);
    let city_key = ik(&interner, "city");
    m.insert(city_key.clone(), InnerValue::Str("LA".into()));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert!(!lens.match_str_eq(city_key, b"NYC"));
}

#[test]
fn match_false_length_differs() {
    let interner = Interner::new();
    let mut m = new_map_wc(1);
    let city_key = ik(&interner, "city");
    m.insert(city_key.clone(), InnerValue::Str("NYC".into()));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert!(!lens.match_str_eq(city_key.clone(), b"NY")); // shorter
    assert!(!lens.match_str_eq(city_key, b"NYCX")); // longer
}

#[test]
fn match_missing_field() {
    let interner = Interner::new();
    let mut m = new_map_wc(1);
    let city_key = ik(&interner, "city");
    let absent_key = ik(&interner, "absent");
    m.insert(city_key, InnerValue::Str("NYC".into()));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert!(!lens.match_str_eq(absent_key, b"NYC"));
}

#[test]
fn match_wrong_type_value() {
    // The field exists but is not a string — match_str_eq returns false
    // (the row does not match the predicate), never panics.
    let interner = Interner::new();
    let mut m = new_map_wc(2);
    let age_key = ik(&interner, "age");
    let flag_key = ik(&interner, "flag");
    m.insert(age_key.clone(), InnerValue::Int(30));
    m.insert(flag_key.clone(), InnerValue::Bool(true));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert!(!lens.match_str_eq(age_key, b"30"));
    assert!(!lens.match_str_eq(flag_key, b"true"));
}

#[test]
fn match_empty_string_field() {
    let interner = Interner::new();
    let mut m = new_map_wc(2);
    let empty_key = ik(&interner, "empty");
    let nonempty_key = ik(&interner, "nonempty");
    m.insert(empty_key.clone(), InnerValue::Str("".into()));
    m.insert(nonempty_key.clone(), InnerValue::Str("x".into()));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert!(lens.match_str_eq(empty_key, b""));
    assert!(!lens.match_str_eq(nonempty_key, b""));
}

#[test]
fn match_skips_non_matching_fields_to_find_target() {
    // Target field is last; match_str_eq must skip earlier fields of any type.
    let interner = Interner::new();
    let mut m = new_map_wc(4);
    m.insert(ik(&interner, "a"), InnerValue::Int(1));
    m.insert(
        ik(&interner, "b"),
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
    );
    m.insert(ik(&interner, "c"), InnerValue::Bin(vec![1, 2, 3]));
    let target_key = ik(&interner, "target");
    m.insert(target_key.clone(), InnerValue::Str("HIT".into()));
    let blob = InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert!(lens.match_str_eq(target_key.clone(), b"HIT"));
    assert!(!lens.match_str_eq(target_key, b"MISS"));
}
