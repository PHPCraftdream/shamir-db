//! Regression checks for interned-record codec and HavingView cards.

use crate::codecs::interned::{inner_value_to_query_value, validate_keys_resolve_interner};
use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{HavingView, RecordRef, RecordView};
use crate::types::common::new_map_wc;
use crate::types::value::{InnerValue, QueryValue};

#[test]
fn rv_types_825c92a8_duplicate_storage_ids_are_rejected() {
    let interner = Interner::new();
    let duplicate = interner.touch_ind("duplicate").unwrap().into_key();
    let distinct = interner.touch_ind("distinct").unwrap().into_key();
    let mut bytes = vec![0x83];
    for (key, value) in [
        (&duplicate, InnerValue::Int(1)),
        (&duplicate, InnerValue::Int(2)),
        (&distinct, InnerValue::Int(3)),
    ] {
        bytes.extend(rmp_serde::to_vec(key).unwrap());
        bytes.extend(value.to_bytes().unwrap());
    }
    let view = RecordView::new(&bytes).unwrap();
    assert!(validate_keys_resolve_interner(&view, &interner).is_err());
}

#[test]
fn rv_types_33abb83b_having_containers_materialize_as_none() {
    let interner = Interner::new();
    let list_key = interner.touch_ind("items").unwrap().into_key();
    let map_key = interner.touch_ind("nested").unwrap().into_key();
    let set_key = interner.touch_ind("members").unwrap().into_key();
    let mut members = crate::types::common::new_set();
    members.insert(QueryValue::Int(1));
    let mut row = new_map_wc(3);
    row.insert(
        "items".to_owned(),
        QueryValue::List(vec![QueryValue::Int(1)]),
    );
    row.insert("nested".to_owned(), QueryValue::Map(new_map_wc(0)));
    row.insert("members".to_owned(), QueryValue::Set(members));
    let row = QueryValue::Map(row);
    let view = HavingView::new(&row, &interner);
    assert_eq!(view.materialize_at(&[list_key]), None);
    assert_eq!(view.materialize_at(&[map_key]), None);
    assert_eq!(view.materialize_at(&[set_key]), None);
}

#[test]
fn rv_types_6c7c9e1e_having_paths_distinguish_scalar_container_missing() {
    let interner = Interner::new();
    let scalar = interner.touch_ind("count").unwrap().into_key();
    let list = interner.touch_ind("items").unwrap().into_key();
    let nested = interner.touch_ind("branch").unwrap().into_key();
    let leaf = interner.touch_ind("leaf").unwrap().into_key();
    let unknown = InternerKey::new(999);
    let mut branch = new_map_wc(1);
    branch.insert("leaf".to_owned(), QueryValue::Int(7));
    let mut row = new_map_wc(3);
    row.insert("count".to_owned(), QueryValue::Int(3));
    row.insert(
        "items".to_owned(),
        QueryValue::List(vec![QueryValue::Int(1)]),
    );
    row.insert("branch".to_owned(), QueryValue::Map(branch));
    let row = QueryValue::Map(row);
    let view = HavingView::new(&row, &interner);
    assert!(view.materialize_at(&[scalar]).is_some());
    assert_eq!(view.materialize_at(&[list]), None);
    assert_eq!(view.materialize_at(&[unknown]), None);
    assert_eq!(view.materialize_at(&[nested, leaf]), None);
}

#[test]
fn rv_types_97a84e4f_recordref_propagates_missing_reverse_key() {
    let interner = Interner::new();
    let mut map = new_map_wc(1);
    map.insert(InternerKey::new(500), InnerValue::Int(1));
    let error = inner_value_to_query_value(&InnerValue::Map(map.clone()), &interner).unwrap_err();
    assert!(format!("{error}").contains("500"));
    let bytes = InnerValue::Map(map).to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    assert!(view.to_query_value(&interner).is_err());
}
