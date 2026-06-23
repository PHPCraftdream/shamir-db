//! Unit tests for [`RecordFields`], [`ViewFields`], and [`OwnedFields`].
//!
//! Covers:
//! - `ViewFields.scalar` by-name via interner matches `RecordView::scalar_at`.
//! - `ViewFields.str` by-name returns the same as `RecordView::str_at`.
//! - `ViewFields.present` classifies values correctly.
//! - `ViewFields.materialize` for scalar and container values.
//! - `OwnedFields` string-keyed lookup for scalar, str, present, materialize.
//! - Absent paths return `None` in both backings.

use shamir_types::core::interner::Interner;
use shamir_types::record_view::{Kind, RecordRef, RecordView, ScalarRef};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::validator::record_fields::{OwnedFields, RecordFields, ViewFields};

/// Intern a string, returning the `InternerKey`.
fn ik(interner: &Interner, s: &str) -> shamir_types::core::interner::InternerKey {
    interner.touch_ind(s).unwrap().into_key()
}

/// Build a representative record and its interner.
/// Record: `{ "name": "alice", "age": 30, "nested": { "x": 7 }, "list": [1, 2] }`
fn build_test_record() -> (InnerValue, Interner) {
    let interner = Interner::default();

    let k_name = ik(&interner, "name");
    let k_age = ik(&interner, "age");
    let k_nested = ik(&interner, "nested");
    let k_x = ik(&interner, "x");
    let k_list = ik(&interner, "list");

    let mut nested = new_map_wc(1);
    nested.insert(k_x, InnerValue::Int(7));

    let mut root = new_map_wc(4);
    root.insert(k_name, InnerValue::Str("alice".to_owned()));
    root.insert(k_age, InnerValue::Int(30));
    root.insert(k_nested, InnerValue::Map(nested));
    root.insert(
        k_list,
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
    );

    (InnerValue::Map(root), interner)
}

/// Build a `QueryValue::Map` equivalent to the test record.
fn build_test_qv() -> QueryValue {
    let mut nested = shamir_types::types::common::new_map();
    nested.insert("x".to_owned(), QueryValue::Int(7));

    let mut root = shamir_types::types::common::new_map();
    root.insert("name".to_owned(), QueryValue::Str("alice".to_owned()));
    root.insert("age".to_owned(), QueryValue::Int(30));
    root.insert("nested".to_owned(), QueryValue::Map(nested));
    root.insert(
        "list".to_owned(),
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Int(2)]),
    );
    QueryValue::Map(root)
}

// ── ViewFields tests ─────────────────────────────────────────────────────

#[test]
fn view_fields_scalar_matches_scalar_at() {
    let (iv, interner) = build_test_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let vf = ViewFields {
        view: &view,
        interner: &interner,
    };

    // Top-level int
    assert_eq!(vf.scalar(&["age"]), Some(ScalarRef::Int(30)));
    // Verify it matches RecordView::scalar_at directly.
    let id_age = interner.get_ind("age").unwrap();
    assert_eq!(view.scalar_at(&[id_age]), Some(ScalarRef::Int(30)));

    // Top-level string
    assert_eq!(vf.scalar(&["name"]), Some(ScalarRef::Str("alice")));

    // Nested scalar
    assert_eq!(vf.scalar(&["nested", "x"]), Some(ScalarRef::Int(7)));
    let id_nested = interner.get_ind("nested").unwrap();
    let id_x = interner.get_ind("x").unwrap();
    assert_eq!(view.scalar_at(&[id_nested, id_x]), Some(ScalarRef::Int(7)));
}

#[test]
fn view_fields_str_returns_string_value() {
    let (iv, interner) = build_test_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let vf = ViewFields {
        view: &view,
        interner: &interner,
    };

    assert_eq!(vf.str(&["name"]), Some("alice"));
    // Non-string field returns None from str().
    assert_eq!(vf.str(&["age"]), None);
}

#[test]
fn view_fields_present_classifies_correctly() {
    let (iv, interner) = build_test_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let vf = ViewFields {
        view: &view,
        interner: &interner,
    };

    assert_eq!(vf.present(&["age"]), Some(Kind::Scalar));
    assert_eq!(vf.present(&["name"]), Some(Kind::Scalar));
    assert_eq!(vf.present(&["nested"]), Some(Kind::Container));
    assert_eq!(vf.present(&["list"]), Some(Kind::Container));
}

#[test]
fn view_fields_materialize_returns_subtree() {
    let (iv, interner) = build_test_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let vf = ViewFields {
        view: &view,
        interner: &interner,
    };

    // Scalar materialise.
    assert_eq!(vf.materialize(&["age"]), Some(InnerValue::Int(30)));

    // Nested scalar materialise.
    assert_eq!(vf.materialize(&["nested", "x"]), Some(InnerValue::Int(7)));
}

#[test]
fn view_fields_absent_returns_none() {
    let (iv, interner) = build_test_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let vf = ViewFields {
        view: &view,
        interner: &interner,
    };

    assert_eq!(vf.scalar(&["no_such_field"]), None);
    assert_eq!(vf.str(&["no_such_field"]), None);
    assert_eq!(vf.present(&["no_such_field"]), None);
    assert_eq!(vf.materialize(&["no_such_field"]), None);
    // Nested absent path.
    assert_eq!(vf.scalar(&["nested", "no_such"]), None);
}

// ── OwnedFields tests ───────────────────────────────────────────────────

#[test]
fn owned_fields_scalar_lookup() {
    let qv = build_test_qv();
    let of = OwnedFields { qv: &qv };

    assert_eq!(of.scalar(&["age"]), Some(ScalarRef::Int(30)));
    assert_eq!(of.scalar(&["name"]), Some(ScalarRef::Str("alice")));
    assert_eq!(of.scalar(&["nested", "x"]), Some(ScalarRef::Int(7)));
}

#[test]
fn owned_fields_str_lookup() {
    let qv = build_test_qv();
    let of = OwnedFields { qv: &qv };

    assert_eq!(of.str(&["name"]), Some("alice"));
    assert_eq!(of.str(&["age"]), None);
}

#[test]
fn owned_fields_present_classifies() {
    let qv = build_test_qv();
    let of = OwnedFields { qv: &qv };

    assert_eq!(of.present(&["age"]), Some(Kind::Scalar));
    assert_eq!(of.present(&["name"]), Some(Kind::Scalar));
    assert_eq!(of.present(&["nested"]), Some(Kind::Container));
    assert_eq!(of.present(&["list"]), Some(Kind::Container));
}

#[test]
fn owned_fields_materialize() {
    let qv = build_test_qv();
    let of = OwnedFields { qv: &qv };

    assert_eq!(of.materialize(&["age"]), Some(InnerValue::Int(30)));
    assert_eq!(of.materialize(&["nested", "x"]), Some(InnerValue::Int(7)));
}

#[test]
fn owned_fields_absent_returns_none() {
    let qv = build_test_qv();
    let of = OwnedFields { qv: &qv };

    assert_eq!(of.scalar(&["no_such"]), None);
    assert_eq!(of.str(&["no_such"]), None);
    assert_eq!(of.present(&["no_such"]), None);
    assert_eq!(of.materialize(&["no_such"]), None);
    assert_eq!(of.scalar(&["nested", "no_such"]), None);
}
