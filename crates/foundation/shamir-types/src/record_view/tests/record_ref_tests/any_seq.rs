//! `any_seq_elem` extraction parity tests.

use super::helpers::{build_record, ik};
use crate::core::interner::InternerKey;
use crate::record_view::{RecordRef, RecordView, ScalarRef};
use crate::types::value::InnerValue;

/// Generic helper: runs `any_seq_elem` on both impls with the same predicate.
fn assert_any_seq_parity(
    iv: &InnerValue,
    view: &RecordView<'_>,
    path: &[InternerKey],
    target: ScalarRef<'_>,
    label: &str,
) {
    let from_tree = iv.any_seq_elem(path, &mut |sr| sr == target);
    let from_lens = view.any_seq_elem(path, &mut |sr| sr == target);
    assert_eq!(
        from_tree, from_lens,
        "any_seq_elem parity failure for '{label}' target={target:?}: tree={from_tree:?}, lens={from_lens:?}"
    );
}

#[test]
fn any_seq_elem_list_match() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let path = [ik(&int, "arr_field")];

    // arr_field = [1, 2] — looking for 1 should return Some(true)
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(1), "arr_match_1");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(1)),
        Some(true)
    );
}

#[test]
fn any_seq_elem_list_no_match() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let path = [ik(&int, "arr_field")];

    // arr_field = [1, 2] — looking for 99 should return Some(false)
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(99), "arr_no_match");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(99)),
        Some(false)
    );
}

#[test]
fn any_seq_elem_empty_list() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let path = [ik(&int, "list_empty")];

    // Empty list — any search returns Some(false)
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(1), "empty_list");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(1)),
        Some(false)
    );
}

#[test]
fn any_seq_elem_non_list_returns_none() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    // Scalar path — not a list/set
    let path = [ik(&int, "int_pos")];
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(12345), "non_list_int");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(12345)),
        None
    );

    // Map path — not a list/set
    let path = [ik(&int, "map_leaf")];
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(99), "non_list_map");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(99)),
        None
    );

    // Missing path
    let path = [ik(&int, "no_such_field")];
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(1), "non_list_missing");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(1)),
        None
    );
}

#[test]
fn any_seq_elem_mixed_list_skips_containers() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();
    let path = [ik(&int, "list_mixed")];

    // list_mixed = [10, "abc", {leaf: 7}, 20]
    // Match scalar 10 — should find it
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(10), "mixed_match_10");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(10)),
        Some(true)
    );

    // Match "abc"
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Str("abc"), "mixed_match_abc");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Str("abc")),
        Some(true)
    );

    // Match 20 (after the container element)
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(20), "mixed_match_20");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(20)),
        Some(true)
    );

    // No match for 99 — the container {leaf: 7} is skipped
    assert_any_seq_parity(&iv, &view, &path, ScalarRef::Int(99), "mixed_no_match");
    assert_eq!(
        iv.any_seq_elem(&path, &mut |sr| sr == ScalarRef::Int(99)),
        Some(false)
    );
}

// ============================================================================
// materialize_at parity
// ============================================================================
