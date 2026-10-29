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
    let vf = ViewFields::new(&view, &interner);

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
    let vf = ViewFields::new(&view, &interner);

    assert_eq!(vf.str(&["name"]), Some("alice"));
    // Non-string field returns None from str().
    assert_eq!(vf.str(&["age"]), None);
}

#[test]
fn view_fields_present_classifies_correctly() {
    let (iv, interner) = build_test_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let vf = ViewFields::new(&view, &interner);

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
    let vf = ViewFields::new(&view, &interner);

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
    let vf = ViewFields::new(&view, &interner);

    assert_eq!(vf.scalar(&["no_such_field"]), None);
    assert_eq!(vf.str(&["no_such_field"]), None);
    assert_eq!(vf.present(&["no_such_field"]), None);
    assert_eq!(vf.materialize(&["no_such_field"]), None);
    // Nested absent path.
    assert_eq!(vf.scalar(&["nested", "no_such"]), None);
}

// ── ViewFields path-cache regression tests (audit group 25 / defect 3) ────
//
// `ViewFields::resolve_path` memoizes the last resolved path so the several
// probes `FieldRule::check`/`check_extended` makes for ONE field within a
// single record don't each re-walk the interner. These tests lock down that
// the memo never resolves to the WRONG field — the exact failure mode a
// caching mistake (e.g. an address-keyed or unconditionally-reused slot)
// would produce.

#[test]
fn view_fields_repeated_probe_same_path_is_consistent() {
    // Probing the SAME path many times in a row (mirrors `FieldRule::check`
    // probing one field 2-4 times per record) must keep returning the same,
    // correct value — not just on the first (cache-populating) call.
    let (iv, interner) = build_test_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let vf = ViewFields::new(&view, &interner);

    for _ in 0..5 {
        assert_eq!(vf.scalar(&["age"]), Some(ScalarRef::Int(30)));
    }
    for _ in 0..5 {
        assert_eq!(vf.scalar(&["nested", "x"]), Some(ScalarRef::Int(7)));
    }
}

#[test]
fn view_fields_alternating_paths_resolve_correctly() {
    // Alternate between two DIFFERENT paths on the SAME `ViewFields`
    // instance — a single-slot memo must fully refresh on every path change,
    // never silently apply the previous path's resolved ids to the new
    // path's segments.
    let (iv, interner) = build_test_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let vf = ViewFields::new(&view, &interner);

    for _ in 0..3 {
        assert_eq!(vf.scalar(&["age"]), Some(ScalarRef::Int(30)));
        assert_eq!(vf.scalar(&["name"]), Some(ScalarRef::Str("alice")));
        assert_eq!(vf.scalar(&["nested", "x"]), Some(ScalarRef::Int(7)));
        // A same-length, different-content path must not be confused with
        // `["nested", "x"]` by a naive length-only cache key.
        assert_eq!(vf.scalar(&["list"]), None); // list is a container, not a scalar
        assert_eq!(
            vf.materialize(&["list"]),
            Some(InnerValue::List(vec![
                InnerValue::Int(1),
                InnerValue::Int(2),
            ]))
        );
    }
}

#[test]
fn view_fields_two_instances_do_not_leak_cache() {
    // Two independently-built records/interners assign DIFFERENT interner
    // ids to the same field name ("age") because interner_b interns THREE
    // filler names before "age" while interner_a's `build_test_record`
    // interns only ONE field ("name") first — a different prior-field
    // count guarantees a different id (interner ids are assigned by
    // insertion order). Resolving "age" on one `ViewFields` must never
    // leak into the other.
    let (iv_a, interner_a) = build_test_record();
    let bytes_a = iv_a.to_bytes().unwrap();
    let view_a = RecordView::new(&bytes_a).unwrap();
    let vf_a = ViewFields::new(&view_a, &interner_a);

    let interner_b = Interner::default();
    let _ = ik(&interner_b, "zzz_filler_1");
    let _ = ik(&interner_b, "zzz_filler_2");
    let _ = ik(&interner_b, "zzz_filler_3");
    let k_age_b = ik(&interner_b, "age");
    let mut root_b = new_map_wc(1);
    root_b.insert(k_age_b, InnerValue::Int(99));
    let iv_b = InnerValue::Map(root_b);
    let bytes_b = iv_b.to_bytes().unwrap();
    let view_b = RecordView::new(&bytes_b).unwrap();
    let vf_b = ViewFields::new(&view_b, &interner_b);

    assert_ne!(
        interner_a.get_ind("age").unwrap(),
        interner_b.get_ind("age").unwrap(),
        "test setup requires diverging ids for the same field name"
    );

    // Probe A, then B, then A again — B's resolution must not corrupt A's
    // cached (or freshly re-resolved) entry, and vice versa.
    assert_eq!(vf_a.scalar(&["age"]), Some(ScalarRef::Int(30)));
    assert_eq!(vf_b.scalar(&["age"]), Some(ScalarRef::Int(99)));
    assert_eq!(vf_a.scalar(&["age"]), Some(ScalarRef::Int(30)));
    assert_eq!(vf_b.scalar(&["age"]), Some(ScalarRef::Int(99)));
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
