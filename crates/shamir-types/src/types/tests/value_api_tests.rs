use crate::types::common::{new_map, TMap};
use crate::types::value::QueryValue;
use crate::types::value_error::ValueError;

// ── helpers ──────────────────────────────────────────────────────────────────

fn map(pairs: &[(&str, QueryValue)]) -> QueryValue {
    let mut m: TMap<String, QueryValue> = new_map();
    for (k, v) in pairs {
        m.insert((*k).to_owned(), v.clone());
    }
    QueryValue::Map(m)
}

// ── get_path ─────────────────────────────────────────────────────────────────

#[test]
fn get_path_hit_shallow() {
    let v = map(&[("name", QueryValue::Str("alice".into()))]);
    assert_eq!(v.get_path("name"), Some(&QueryValue::Str("alice".into())));
}

#[test]
fn get_path_hit_nested() {
    let inner = map(&[("city", QueryValue::Str("NY".into()))]);
    let v = map(&[("address", inner)]);
    assert_eq!(
        v.get_path("address.city"),
        Some(&QueryValue::Str("NY".into()))
    );
}

#[test]
fn get_path_miss_absent_key() {
    let v = map(&[("a", QueryValue::Int(1))]);
    assert_eq!(v.get_path("b"), None);
}

#[test]
fn get_path_miss_absent_nested_key() {
    let inner = map(&[("x", QueryValue::Int(1))]);
    let v = map(&[("a", inner)]);
    assert_eq!(v.get_path("a.y"), None);
}

#[test]
fn get_path_non_map_intermediate_returns_none() {
    // "a" is an Int, not a Map — descending further returns None.
    let v = map(&[("a", QueryValue::Int(42))]);
    assert_eq!(v.get_path("a.b"), None);
}

#[test]
fn get_path_triple_nesting() {
    let level3 = map(&[("val", QueryValue::Bool(true))]);
    let level2 = map(&[("c", level3)]);
    let level1 = map(&[("b", level2)]);
    let root = map(&[("a", level1)]);
    assert_eq!(root.get_path("a.b.c.val"), Some(&QueryValue::Bool(true)));
}

// ── get_path_mut ──────────────────────────────────────────────────────────────

#[test]
fn get_path_mut_hit_and_mutate() {
    let mut v = map(&[("score", QueryValue::Int(0))]);
    if let Some(node) = v.get_path_mut("score") {
        *node = QueryValue::Int(99);
    }
    assert_eq!(v.get_path("score"), Some(&QueryValue::Int(99)));
}

#[test]
fn get_path_mut_miss_returns_none() {
    let mut v = map(&[("a", QueryValue::Int(1))]);
    assert!(v.get_path_mut("missing").is_none());
}

#[test]
fn get_path_mut_non_map_intermediate_returns_none() {
    let mut v = map(&[("a", QueryValue::Str("leaf".into()))]);
    assert!(v.get_path_mut("a.child").is_none());
}

// ── set_path ──────────────────────────────────────────────────────────────────

#[test]
fn set_path_creates_leaf_in_existing_map() {
    let mut v = map(&[("x", QueryValue::Int(1))]);
    let prev = v.set_path("y", QueryValue::Str("new".into())).unwrap();
    assert_eq!(prev, None);
    assert_eq!(v.get_path("y"), Some(&QueryValue::Str("new".into())));
}

#[test]
fn set_path_returns_previous_value_on_overwrite() {
    let mut v = map(&[("k", QueryValue::Int(10))]);
    let prev = v.set_path("k", QueryValue::Int(20)).unwrap();
    assert_eq!(prev, Some(QueryValue::Int(10)));
    assert_eq!(v.get_path("k"), Some(&QueryValue::Int(20)));
}

#[test]
fn set_path_creates_intermediate_map_nodes() {
    let mut v = map(&[]);
    v.set_path("a.b.c", QueryValue::Bool(true)).unwrap();
    assert_eq!(v.get_path("a.b.c"), Some(&QueryValue::Bool(true)));
    // Intermediate "a" and "a.b" must be Maps.
    assert!(v.get_path("a").map(|n| n.is_map()).unwrap_or(false));
    assert!(v.get_path("a.b").map(|n| n.is_map()).unwrap_or(false));
}

#[test]
fn set_path_error_on_non_map_intermediate() {
    // "a" exists but is an Int, not a Map — set_path must error.
    let mut v = map(&[("a", QueryValue::Int(5))]);
    let result = v.set_path("a.b", QueryValue::Str("x".into()));
    assert!(
        matches!(result, Err(ValueError::NotAMap { .. })),
        "expected NotAMap, got {:?}",
        result
    );
}

#[test]
fn set_path_single_segment_on_non_map_root_errors() {
    // Root is an Int — cannot set a key on it.
    let mut v = QueryValue::Int(0);
    let result = v.set_path("k", QueryValue::Bool(false));
    assert!(
        matches!(result, Err(ValueError::NotAMap { .. })),
        "expected NotAMap, got {:?}",
        result
    );
}

// ── Index<&str> ───────────────────────────────────────────────────────────────
//
// The existing `impl Index<&str> for Value<String>` uses a sentinel-null
// strategy (returns &Null on miss / non-map) rather than panicking.  This is
// intentional: multiple engine call-sites rely on the silent-null behaviour
// (e.g. `record["name"].as_str()`).  The tests below document this contract.

#[test]
fn index_str_hit_returns_value() {
    let v = map(&[("age", QueryValue::Int(30))]);
    assert_eq!(v["age"], QueryValue::Int(30));
}

#[test]
fn index_str_miss_returns_null_sentinel() {
    let v = map(&[("a", QueryValue::Int(1))]);
    assert_eq!(v["missing"], QueryValue::Null);
}

#[test]
fn index_str_non_map_returns_null_sentinel() {
    let v = QueryValue::Int(42);
    assert_eq!(v["any"], QueryValue::Null);
}

// ── Index<usize> ──────────────────────────────────────────────────────────────

#[test]
fn index_usize_hit_returns_element() {
    let v = QueryValue::List(vec![QueryValue::Int(7), QueryValue::Bool(true)]);
    assert_eq!(v[0], QueryValue::Int(7));
    assert_eq!(v[1], QueryValue::Bool(true));
}

#[test]
fn index_usize_out_of_bounds_returns_null_sentinel() {
    let v = QueryValue::List(vec![QueryValue::Int(1)]);
    assert_eq!(v[99], QueryValue::Null);
}

#[test]
fn index_usize_non_list_returns_null_sentinel() {
    let v = QueryValue::Int(5);
    assert_eq!(v[0], QueryValue::Null);
}

// ── as_str_or / as_i64_or ────────────────────────────────────────────────────

#[test]
fn as_str_or_hit() {
    let v = QueryValue::Str("hello".into());
    assert_eq!(v.as_str_or("default"), "hello");
}

#[test]
fn as_str_or_miss_returns_default() {
    let v = QueryValue::Int(42);
    assert_eq!(v.as_str_or("fallback"), "fallback");
}

#[test]
fn as_str_or_null_returns_default() {
    let v = QueryValue::Null;
    assert_eq!(v.as_str_or("fb"), "fb");
}

#[test]
fn as_i64_or_hit() {
    let v = QueryValue::Int(-7);
    assert_eq!(v.as_i64_or(0), -7);
}

#[test]
fn as_i64_or_miss_returns_default() {
    let v = QueryValue::Str("x".into());
    assert_eq!(v.as_i64_or(42), 42);
}

#[test]
fn as_i64_or_null_returns_default() {
    let v = QueryValue::Null;
    assert_eq!(v.as_i64_or(99), 99);
}

// ── require_str ───────────────────────────────────────────────────────────────

#[test]
fn require_str_hit() {
    let v = QueryValue::Str("world".into());
    assert_eq!(v.require_str("field").unwrap(), "world");
}

#[test]
fn require_str_wrong_type_returns_type_mismatch() {
    let v = QueryValue::Int(1);
    let err = v.require_str("user.age").unwrap_err();
    assert!(
        matches!(&err, ValueError::TypeMismatch { path, expected, got }
            if path == "user.age" && *expected == "str" && *got == "int"),
        "unexpected error variant: {:?}",
        err
    );
}

#[test]
fn require_str_null_returns_type_mismatch() {
    let v = QueryValue::Null;
    let err = v.require_str("x").unwrap_err();
    assert!(matches!(err, ValueError::TypeMismatch { got, .. } if got == "null"));
}

// ── ValueError display ────────────────────────────────────────────────────────

#[test]
fn value_error_type_mismatch_display() {
    let e = ValueError::TypeMismatch {
        path: "user.name".into(),
        expected: "str",
        got: "int",
    };
    let msg = e.to_string();
    assert!(msg.contains("user.name"), "missing path in: {msg}");
    assert!(msg.contains("str"), "missing expected in: {msg}");
    assert!(msg.contains("int"), "missing got in: {msg}");
}

#[test]
fn value_error_path_not_found_display() {
    let e = ValueError::PathNotFound {
        path: "a.b.c".into(),
    };
    let msg = e.to_string();
    assert!(msg.contains("a.b.c"), "missing path in: {msg}");
}

#[test]
fn value_error_not_a_map_display() {
    let e = ValueError::NotAMap {
        path: "root.leaf".into(),
    };
    let msg = e.to_string();
    assert!(msg.contains("root.leaf"), "missing path in: {msg}");
}
