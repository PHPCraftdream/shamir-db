//! `materialize_at` parity tests.

use super::helpers::{assert_materialize_parity, build_record, ik};
use crate::record_view::{RecordRef, RecordView};
use crate::types::value::InnerValue;

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
