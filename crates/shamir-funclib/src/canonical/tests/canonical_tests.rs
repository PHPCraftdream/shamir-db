//! Determinism + self-reference-break tests for the canonical record hash.

use crate::canonical::{canonical_hash, register, PREV_HASH_FIELD};
use crate::registry::ScalarRegistry;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{QueryValue, Value};
use std::str::FromStr;

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
    // under the folder-qualified name. Under QueryValue ABI the argument is
    // string-keyed, so no interner is needed.
    let mut r = ScalarRegistry::new();
    r.in_folder("crypto", register);

    let arg = QueryValue::Str("Alice".to_owned());
    let got = r
        .call("crypto/canonical_hash", std::slice::from_ref(&arg))
        .unwrap();
    match got {
        QueryValue::Str(h) => assert_eq!(h.len(), 64, "BLAKE3 hex digest is 64 chars"),
        other => panic!("expected Str digest, got {other:?}"),
    }

    // Arity is enforced by the registry.
    assert_eq!(
        r.call("crypto/canonical_hash", &[]).unwrap_err().code,
        "arity"
    );
}

// ── COR-1 fix: round-trip invariance for Dec and Big ─────────────────────────

/// Dec is serialised to the wire as `d.to_string()` (Value::Serialize), so
/// after a msgpack round-trip it becomes Value::Str(d.to_string()).
/// canonical_hash must be identical before and after that round-trip.
#[test]
fn dec_hash_equals_its_wire_str_hash() {
    let d = Decimal::from_str("1.50").unwrap();
    let wire_str = d.to_string(); // exactly what Serialize produces

    let hash_dec = canonical_hash(&QueryValue::Dec(d));
    let hash_str = canonical_hash(&QueryValue::Str(wire_str));

    assert_eq!(
        hash_dec, hash_str,
        "Dec hash must equal the hash of its wire-string form (round-trip invariance)"
    );
}

/// Same invariant for BigInt.
#[test]
fn big_hash_equals_its_wire_str_hash() {
    let b: BigInt = "123456789012345678901234567890".parse().unwrap();
    let wire_str = b.to_string();

    let hash_big = canonical_hash(&QueryValue::Big(b));
    let hash_str = canonical_hash(&QueryValue::Str(wire_str));

    assert_eq!(
        hash_big, hash_str,
        "Big hash must equal the hash of its wire-string form (round-trip invariance)"
    );
}

/// Full end-to-end round-trip: record with a Dec field → msgpack bytes →
/// deserialise (Dec becomes Str) → canonical_hash unchanged.
#[test]
fn dec_field_hash_survives_msgpack_round_trip() {
    let d = Decimal::from_str("3.14").unwrap();
    let record_before = map(&[("id", Value::Int(1)), ("price", QueryValue::Dec(d))]);

    // Serialise → deserialise: Dec survives as Str on the wire.
    let bytes = record_before.to_bytes().expect("serialise");
    let record_after: QueryValue = QueryValue::from_bytes(&bytes).expect("deserialise");

    assert_eq!(
        canonical_hash(&record_before),
        canonical_hash(&record_after),
        "canonical_hash must be identical before and after a msgpack round-trip for Dec fields"
    );
}

/// Full end-to-end round-trip: record with a Big field → msgpack bytes →
/// deserialise (Big becomes Str) → canonical_hash unchanged.
#[test]
fn big_field_hash_survives_msgpack_round_trip() {
    let b: BigInt = "99999999999999999999".parse().unwrap();
    let record_before = map(&[("id", Value::Int(2)), ("amount", QueryValue::Big(b))]);

    let bytes = record_before.to_bytes().expect("serialise");
    let record_after: QueryValue = QueryValue::from_bytes(&bytes).expect("deserialise");

    assert_eq!(
        canonical_hash(&record_before),
        canonical_hash(&record_after),
        "canonical_hash must be identical before and after a msgpack round-trip for Big fields"
    );
}
