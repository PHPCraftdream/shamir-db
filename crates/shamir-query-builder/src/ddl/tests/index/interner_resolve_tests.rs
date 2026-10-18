use shamir_collections::TFxMap;

// `interner_resolve` is a private `mod` under `ddl` and re-exported via
// `pub use interner_resolve::*` at `crate::ddl`. The re-export is the public
// path, so import the names from there (NOT `super::interner_resolve`, which
// would require a `pub mod`).
use crate::ddl::{resolve_field_path, resolve_field_paths};

#[test]
fn resolve_known_name() {
    let mut map: TFxMap<&str, u64> = TFxMap::default();
    map.insert("age", 7);
    let resolver = |n: &str| map.get(n).copied();
    assert_eq!(resolve_field_path(&resolver, "age"), Some(7));
}

#[test]
fn resolve_unknown_returns_none() {
    let map: TFxMap<&str, u64> = TFxMap::default();
    let resolver = |n: &str| map.get(n).copied();
    assert_eq!(resolve_field_path(&resolver, "missing"), None);
}

#[test]
fn numeric_string_is_string_not_id() {
    // §9.4: "42" is the STRING "42" — it resolves via the map, NOT to the
    // integer 42. If the map has no entry for the string "42", we get None,
    // never Some(42).
    let mut map: TFxMap<&str, u64> = TFxMap::default();
    map.insert("42", 99); // the field literally named "42" has id 99
    let resolver = |n: &str| map.get(n).copied();
    assert_eq!(resolve_field_path(&resolver, "42"), Some(99));
    assert_ne!(resolve_field_path(&resolver, "42"), Some(42));

    // And a numeric string with no entry returns None, NOT its parse.
    let empty: TFxMap<&str, u64> = TFxMap::default();
    let resolver2 = |n: &str| empty.get(n).copied();
    assert_eq!(resolve_field_path(&resolver2, "42"), None);
}

#[test]
fn resolve_batch_drops_unknowns() {
    let mut map: TFxMap<&str, u64> = TFxMap::default();
    map.insert("a", 1);
    map.insert("c", 3);
    let resolver = |n: &str| map.get(n).copied();
    let out = resolve_field_paths(&resolver, vec!["a", "b", "c"]);
    assert_eq!(out, vec![("a", 1), ("c", 3)]);
}
