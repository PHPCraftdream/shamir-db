//! Determinism + self-reference-break tests for the canonical record hash.

use crate::canonical::{canonical_hash, register, PREV_HASH_FIELD};
use crate::registry::ScalarRegistry;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{InnerValue, QueryValue, Value};

/// Build a string-keyed map from `(key, value)` pairs in the given order.
fn map(pairs: &[(&str, QueryValue)]) -> QueryValue {
    let mut m = new_map();
    for (k, v) in pairs {
        m.insert((*k).to_owned(), v.clone());
    }
    QueryValue::Map(m)
}

fn s(v: &str) -> QueryValue {
    Value::Str(v.to_owned())
}

#[test]
fn key_order_does_not_change_hash() {
    // Same logical content, different insertion order of keys.
    let a = map(&[
        ("name", s("Alice")),
        ("age", Value::Int(30)),
        ("city", s("Haifa")),
    ]);
    let b = map(&[
        ("city", s("Haifa")),
        ("name", s("Alice")),
        ("age", Value::Int(30)),
    ]);

    assert_eq!(
        canonical_hash(&a),
        canonical_hash(&b),
        "key order must not affect the canonical hash"
    );
}

#[test]
fn nested_key_order_does_not_change_hash() {
    let inner_1 = map(&[("x", Value::Int(1)), ("y", Value::Int(2))]);
    let inner_2 = map(&[("y", Value::Int(2)), ("x", Value::Int(1))]);

    let a = map(&[("id", Value::Int(7)), ("pos", inner_1)]);
    let b = map(&[("pos", inner_2), ("id", Value::Int(7))]);

    assert_eq!(
        canonical_hash(&a),
        canonical_hash(&b),
        "nested map key order must not affect the canonical hash"
    );
}

#[test]
fn data_change_changes_hash() {
    let a = map(&[("name", s("Alice")), ("age", Value::Int(30))]);
    let b = map(&[("name", s("Alice")), ("age", Value::Int(31))]);
    assert_ne!(
        canonical_hash(&a),
        canonical_hash(&b),
        "changing a value must change the hash"
    );

    // Renaming a key must also change the hash.
    let c = map(&[("name", s("Alice")), ("years", Value::Int(30))]);
    assert_ne!(
        canonical_hash(&a),
        canonical_hash(&c),
        "renaming a key must change the hash"
    );
}

#[test]
fn array_order_is_significant() {
    let a = QueryValue::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    let b = QueryValue::List(vec![Value::Int(3), Value::Int(2), Value::Int(1)]);
    assert_ne!(
        canonical_hash(&a),
        canonical_hash(&b),
        "array element order must be significant"
    );
}

#[test]
fn prev_hash_field_does_not_affect_hash() {
    let base = map(&[("name", s("Alice")), ("age", Value::Int(30))]);

    let with_prev = map(&[
        ("name", s("Alice")),
        ("age", Value::Int(30)),
        (PREV_HASH_FIELD, s("deadbeef")),
    ]);

    let with_other_prev = map(&[
        ("name", s("Alice")),
        ("age", Value::Int(30)),
        (PREV_HASH_FIELD, s("0123456789")),
    ]);

    assert_eq!(
        canonical_hash(&base),
        canonical_hash(&with_prev),
        "the reserved _prev_hash field must be excluded from the hash"
    );
    assert_eq!(
        canonical_hash(&with_prev),
        canonical_hash(&with_other_prev),
        "the value of _prev_hash must not affect the hash"
    );
}

#[test]
fn prev_hash_excluded_only_at_top_level() {
    // A nested _prev_hash IS part of the content (only the top level is the
    // CAS control field) and therefore changes the hash.
    let nested_a = map(&[("meta", map(&[(PREV_HASH_FIELD, s("aaa"))]))]);
    let nested_b = map(&[("meta", map(&[(PREV_HASH_FIELD, s("bbb"))]))]);
    assert_ne!(
        canonical_hash(&nested_a),
        canonical_hash(&nested_b),
        "_prev_hash nested below the top level is ordinary content"
    );
}

#[test]
fn type_tags_disambiguate_payloads() {
    // Empty string vs empty list must not collide.
    let empty_str = QueryValue::Str(String::new());
    let empty_list = QueryValue::List(Vec::new());
    assert_ne!(
        canonical_hash(&empty_str),
        canonical_hash(&empty_list),
        "different types with empty payloads must hash differently"
    );

    // Int 1 vs Bool true must not collide.
    assert_ne!(
        canonical_hash(&Value::<String>::Int(1)),
        canonical_hash(&Value::<String>::Bool(true)),
        "Int and Bool must hash differently"
    );
}

#[test]
fn hash_is_lowercase_hex_64_chars() {
    let v = map(&[("name", s("Alice"))]);
    let h = canonical_hash(&v);
    assert_eq!(h.len(), 64, "BLAKE3 hex digest is 64 chars");
    assert!(
        h.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "digest must be lowercase hex, got {h}"
    );
}

#[test]
fn scalar_registered_under_crypto_folder() {
    // The scalar wraps canonical_hash and returns a Str; verify it dispatches
    // under the folder-qualified name. The scalar runs on InnerValue, so we
    // feed it a non-map InnerValue (no interner needed) and check the shape.
    let mut r = ScalarRegistry::new();
    r.in_folder("crypto", register);

    let arg = InnerValue::Str("Alice".to_owned());
    let got = r
        .call("crypto/canonical_hash", std::slice::from_ref(&arg))
        .unwrap();
    match got {
        InnerValue::Str(h) => assert_eq!(h.len(), 64, "BLAKE3 hex digest is 64 chars"),
        other => panic!("expected Str digest, got {other:?}"),
    }

    // Arity is enforced by the registry.
    assert_eq!(
        r.call("crypto/canonical_hash", &[]).unwrap_err().code,
        "arity"
    );
}
