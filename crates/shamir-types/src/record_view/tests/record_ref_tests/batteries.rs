//! Whole-record parity batteries (kind / str_at / exists_at / is_null_at).

use super::helpers::{
    assert_exists_parity, assert_is_null_parity, assert_kind_parity, assert_parity,
    assert_str_at_parity, build_record, ik, probe,
};
use crate::core::interner::InternerKey;
use crate::record_view::{Kind, RecordRef, RecordView, ScalarRef};

#[test]
fn parity_battery() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();

    let cases: Vec<(Vec<InternerKey>, &str, Option<ScalarRef<'_>>)> = vec![
        (vec![ik(&int, "null_field")], "null", Some(ScalarRef::Null)),
        (
            vec![ik(&int, "bool_true")],
            "bool_t",
            Some(ScalarRef::Bool(true)),
        ),
        (
            vec![ik(&int, "bool_false")],
            "bool_f",
            Some(ScalarRef::Bool(false)),
        ),
        (
            vec![ik(&int, "int_pos")],
            "int+",
            Some(ScalarRef::Int(12345)),
        ),
        (
            vec![ik(&int, "int_neg")],
            "int-",
            Some(ScalarRef::Int(-999)),
        ),
        (vec![ik(&int, "int_zero")], "int0", Some(ScalarRef::Int(0))),
        (vec![ik(&int, "f64_val")], "f64", Some(ScalarRef::F64(1.23))),
        (
            vec![ik(&int, "str_val")],
            "str",
            Some(ScalarRef::Str("shamir")),
        ),
        (
            vec![ik(&int, "str_empty")],
            "str_e",
            Some(ScalarRef::Str("")),
        ),
        (
            vec![ik(&int, "bin_val")],
            "bin",
            Some(ScalarRef::Bin(&[0xDE, 0xAD, 0xBE, 0xEF])),
        ),
        (
            vec![ik(&int, "bin_empty")],
            "bin_e",
            Some(ScalarRef::Bin(&[])),
        ),
        (
            vec![ik(&int, "nested"), ik(&int, "inner_field")],
            "nest1",
            Some(ScalarRef::Int(42)),
        ),
        (
            vec![ik(&int, "nested"), ik(&int, "deep"), ik(&int, "leaf")],
            "nest3",
            Some(ScalarRef::Str("hello")),
        ),
        (vec![ik(&int, "no_such_field")], "miss", None),
        (
            vec![ik(&int, "int_pos"), ik(&int, "inner_field")],
            "thru_int",
            None,
        ),
        (vec![ik(&int, "arr_field")], "arr_leaf", None),
        (vec![ik(&int, "map_leaf")], "map_leaf", None),
        (vec![], "empty", None),
    ];

    for (path, label, expected_tree) in &cases {
        // Assert tree gives expected value.
        let from_tree = probe(&iv, path);
        assert_eq!(
            &from_tree, expected_tree,
            "tree result for '{label}' differs from expected"
        );
        // Assert parity between tree and lens.
        assert_parity(&iv, &bytes, path, label);
    }
}

// ============================================================================
// present_kind_at parity
// ============================================================================

#[test]
fn kind_parity_battery() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    // (path, label, expected_tree_kind)
    let cases: Vec<(Vec<InternerKey>, &str, Option<Kind>)> = vec![
        (vec![ik(&int, "null_field")], "null", Some(Kind::Null)),
        (vec![ik(&int, "bool_true")], "bool_t", Some(Kind::Scalar)),
        (vec![ik(&int, "int_pos")], "int+", Some(Kind::Scalar)),
        (vec![ik(&int, "f64_val")], "f64", Some(Kind::Scalar)),
        (vec![ik(&int, "str_val")], "str", Some(Kind::Scalar)),
        (vec![ik(&int, "bin_val")], "bin", Some(Kind::Scalar)),
        (
            vec![ik(&int, "dec_field")],
            "dec",
            Some(Kind::NonComparable),
        ),
        (vec![ik(&int, "arr_field")], "arr", Some(Kind::Container)),
        (vec![ik(&int, "map_leaf")], "map", Some(Kind::Container)),
        (
            vec![ik(&int, "nested")],
            "nested_map",
            Some(Kind::Container),
        ),
        (
            vec![ik(&int, "nested"), ik(&int, "inner_field")],
            "nested.inner",
            Some(Kind::Scalar),
        ),
        (
            vec![ik(&int, "nested"), ik(&int, "deep")],
            "nested.deep",
            Some(Kind::Container),
        ),
        (vec![ik(&int, "no_such_field")], "missing", None),
        (
            vec![ik(&int, "int_pos"), ik(&int, "inner_field")],
            "thru_int",
            None,
        ),
        (vec![], "empty", None),
    ];

    for (path, label, expected_tree) in &cases {
        let from_tree = iv.present_kind_at(path);
        assert_eq!(
            &from_tree, expected_tree,
            "tree kind for '{label}': got {from_tree:?}, expected {expected_tree:?}"
        );
        assert_kind_parity(&iv, &view, path, label);
    }
}

// ============================================================================
// str_at parity
// ============================================================================

#[test]
fn str_at_parity_battery() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    // (path, label, expected)
    let cases: Vec<(Vec<InternerKey>, &str, Option<&str>)> = vec![
        (vec![ik(&int, "str_val")], "str", Some("shamir")),
        (vec![ik(&int, "str_empty")], "str_empty", Some("")),
        (vec![ik(&int, "int_pos")], "int_not_str", None),
        (vec![ik(&int, "null_field")], "null_not_str", None),
        (vec![ik(&int, "bool_true")], "bool_not_str", None),
        (vec![ik(&int, "f64_val")], "f64_not_str", None),
        (vec![ik(&int, "bin_val")], "bin_not_str", None),
        (vec![ik(&int, "arr_field")], "arr_not_str", None),
        (vec![ik(&int, "map_leaf")], "map_not_str", None),
        (vec![ik(&int, "no_such_field")], "missing", None),
        (
            vec![ik(&int, "nested"), ik(&int, "deep"), ik(&int, "leaf")],
            "nested_str",
            Some("hello"),
        ),
        (vec![], "empty_path", None),
    ];

    for (path, label, expected) in &cases {
        let from_tree = iv.str_at(path);
        assert_eq!(
            &from_tree, expected,
            "tree str_at for '{label}': got {from_tree:?}, expected {expected:?}"
        );
        assert_str_at_parity(&iv, &view, path, label);
    }
}

// ============================================================================
// exists_at / is_null_at parity
// ============================================================================

#[test]
fn exists_at_parity_battery() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    let cases: Vec<(Vec<InternerKey>, &str, bool)> = vec![
        (vec![ik(&int, "null_field")], "null_exists", true),
        (vec![ik(&int, "int_pos")], "int_exists", true),
        (vec![ik(&int, "str_val")], "str_exists", true),
        (vec![ik(&int, "arr_field")], "arr_exists", true),
        (vec![ik(&int, "map_leaf")], "map_exists", true),
        (vec![ik(&int, "dec_field")], "dec_exists", true),
        (
            vec![ik(&int, "nested"), ik(&int, "inner_field")],
            "nested_exists",
            true,
        ),
        (vec![ik(&int, "no_such_field")], "missing_not_exists", false),
        (
            vec![ik(&int, "int_pos"), ik(&int, "inner_field")],
            "thru_scalar",
            false,
        ),
        (vec![], "empty_path", false),
    ];

    for (path, label, expected) in &cases {
        let from_tree = iv.exists_at(path);
        assert_eq!(
            from_tree, *expected,
            "tree exists_at for '{label}': got {from_tree}, expected {expected}"
        );
        assert_exists_parity(&iv, &view, path, label);
    }
}

#[test]
fn is_null_at_parity_battery() {
    let (iv, int) = build_record();
    let bytes = iv.to_bytes().unwrap();
    let view = RecordView::new(&bytes).unwrap();

    let cases: Vec<(Vec<InternerKey>, &str, bool)> = vec![
        (vec![ik(&int, "null_field")], "null_is_null", true),
        (vec![ik(&int, "int_pos")], "int_not_null", false),
        (vec![ik(&int, "str_val")], "str_not_null", false),
        (vec![ik(&int, "arr_field")], "arr_not_null", false),
        (vec![ik(&int, "no_such_field")], "missing_is_null", true),
        (vec![], "empty_path_is_null", true),
        (
            vec![ik(&int, "nested"), ik(&int, "inner_field")],
            "nested_not_null",
            false,
        ),
    ];

    for (path, label, expected) in &cases {
        let from_tree = iv.is_null_at(path);
        assert_eq!(
            from_tree, *expected,
            "tree is_null_at for '{label}': got {from_tree}, expected {expected}"
        );
        assert_is_null_parity(&iv, &view, path, label);
    }
}

// ============================================================================
// any_seq_elem parity
// ============================================================================
