//! Direct HavingView contract tests.

use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{HavingView, RecordRef};
use crate::types::common::new_map_wc;
use crate::types::value::QueryValue;

#[test]
fn rv_types_33abb83b_container_materialization_is_none() {
    let interner = Interner::new();
    let list_key = interner.touch_ind("items").unwrap().into_key();
    let map_key = interner.touch_ind("nested").unwrap().into_key();
    let set_key = interner.touch_ind("members").unwrap().into_key();
    let nested_key = interner.touch_ind("branch").unwrap().into_key();
    let leaf_key = interner.touch_ind("leaf").unwrap().into_key();
    let mut members = crate::types::common::new_set();
    members.insert(QueryValue::Int(1));
    let mut branch = new_map_wc(1);
    branch.insert("leaf".to_owned(), QueryValue::Int(9));
    let mut row = new_map_wc(4);
    row.insert(
        "items".to_owned(),
        QueryValue::List(vec![QueryValue::Int(1)]),
    );
    row.insert("nested".to_owned(), QueryValue::Map(new_map_wc(0)));
    row.insert("members".to_owned(), QueryValue::Set(members));
    row.insert("branch".to_owned(), QueryValue::Map(branch));
    let row = QueryValue::Map(row);
    let view = HavingView::new(&row, &interner);
    assert_eq!(view.materialize_at(&[list_key]), None);
    assert_eq!(view.materialize_at(&[map_key]), None);
    assert_eq!(view.materialize_at(&[set_key]), None);
    assert_eq!(view.materialize_at(&[nested_key, leaf_key]), None);
}

#[test]
fn rv_types_6c7c9e1e_scalar_container_and_unknown_path_contract() {
    let interner = Interner::new();
    let scalar = interner.touch_ind("count").unwrap().into_key();
    let list = interner.touch_ind("items").unwrap().into_key();
    let unknown = InternerKey::new(999);
    let mut row = new_map_wc(2);
    row.insert("count".to_owned(), QueryValue::Int(3));
    row.insert(
        "items".to_owned(),
        QueryValue::List(vec![QueryValue::Int(1)]),
    );
    let row = QueryValue::Map(row);
    let view = HavingView::new(&row, &interner);
    assert_eq!(
        view.materialize_at(&[scalar]),
        Some(crate::types::value::InnerValue::Int(3))
    );
    assert_eq!(view.materialize_at(&[list]), None);
    assert_eq!(view.materialize_at(&[unknown]), None);
}
