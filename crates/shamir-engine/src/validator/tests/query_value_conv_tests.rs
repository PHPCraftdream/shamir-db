//! Round-trip identity proof for the validator record conversion.
//!
//! W1 (insert-path validator cutover) is only safe if the round-trip the
//! validator currently performs is IDENTITY for the value shapes inserts
//! carry, i.e.
//!
//! ```text
//! inner_to_query_value_with(query_value_to_inner_with(qv, intern_fn), resolve) == qv
//! ```
//!
//! If that holds, feeding the already-resolved `QueryValue` directly to the
//! validator is behaviour-identical to the current
//! `InnerValue -> QueryValue` de-intern of the freshly-built record. If any
//! shape is NOT identity (value normalization, coercion, key reorder, set
//! dedup that loses information), W1 is OFF and this test must fail so the
//! divergence is surfaced rather than silently breaking validator semantics.
//!
//! The `intern_fn` / `resolve` pair here mirrors the closures the write path
//! and `run_validators_resolved` build: `intern_fn` interns a `&str` key into
//! an `InternerKey` via a fresh `Interner`; `resolve` de-interns via the same
//! interner's `with_str`. The intern/deintern pair is a documented inverse
//! (see `shamir_types::core::interner`).

use std::str::FromStr;

use shamir_types::codecs::interned::query_value_to_inner_with;
use shamir_types::codecs::CodecError;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::value::{QueryValue, Value};

use crate::validator::inner_to_query_value_with;

/// Build a (intern_fn, resolve) pair backed by a single shared `Interner`,
/// matching the write-path / `run_validators_resolved` setup.
struct Resolver {
    interner: Interner,
}

impl Resolver {
    fn new() -> Self {
        Self {
            interner: Interner::new(),
        }
    }

    fn intern_fn(&self) -> impl Fn(&str) -> Result<InternerKey, CodecError> + '_ {
        |key: &str| {
            self.interner
                .touch_ind(key)
                .map(|t| t.into_key())
                .map_err(|e| CodecError::Decode(format!("Failed to intern key '{}': {}", key, e)))
        }
    }

    fn resolve_fn(&self) -> impl Fn(&InternerKey) -> Option<String> + '_ {
        |key: &InternerKey| self.interner.with_str(key, |s| s.to_string())
    }
}

/// Assert `inner_to_query_value_with(query_value_to_inner_with(qv)) == qv`
/// for one value, failing loudly with the divergent shape on mismatch.
fn assert_round_trip_identity(qv: &QueryValue, label: &str) {
    let r = Resolver::new();
    let inner = query_value_to_inner_with(qv, &r.intern_fn()).expect("intern must succeed");
    let back = inner_to_query_value_with(&inner, &r.resolve_fn()).expect("resolve must succeed");
    assert_eq!(&back, qv, "round-trip NOT identity for shape: {}", label);
}

#[test]
fn round_trip_identity_scalars() {
    assert_round_trip_identity(&QueryValue::Null, "Null");
    assert_round_trip_identity(&QueryValue::Bool(true), "Bool(true)");
    assert_round_trip_identity(&QueryValue::Bool(false), "Bool(false)");
    assert_round_trip_identity(&QueryValue::Int(0), "Int(0)");
    assert_round_trip_identity(&QueryValue::Int(-42), "Int(-42)");
    assert_round_trip_identity(&QueryValue::Int(i64::MAX), "Int(i64::MAX)");
    assert_round_trip_identity(&QueryValue::Int(i64::MIN), "Int(i64::MIN)");
    assert_round_trip_identity(&QueryValue::F64(0.0), "F64(0.0)");
    assert_round_trip_identity(&QueryValue::F64(-2.5), "F64(-2.5)");
    assert_round_trip_identity(&QueryValue::F64(1e308), "F64(1e308)");
    // NaN: Value's PartialEq treats NaN == NaN as true (value.rs).
    assert_round_trip_identity(&QueryValue::F64(f64::NAN), "F64(NaN)");
    assert_round_trip_identity(&QueryValue::F64(f64::INFINITY), "F64(+inf)");
    assert_round_trip_identity(&QueryValue::F64(f64::NEG_INFINITY), "F64(-inf)");
    assert_round_trip_identity(&QueryValue::Str("".to_string()), "Str(empty)");
    assert_round_trip_identity(&QueryValue::Str("hello".to_string()), "Str(hello)");
    assert_round_trip_identity(
        &QueryValue::Str("Ünïcödé 🦀 key".to_string()),
        "Str(unicode)",
    );
    assert_round_trip_identity(
        &QueryValue::Dec(rust_decimal::Decimal::from_str("123.456").unwrap()),
        "Dec(123.456)",
    );
    assert_round_trip_identity(
        &QueryValue::Big(num_bigint::BigInt::from(99999999999999_i64)),
        "Big(large)",
    );
    assert_round_trip_identity(&QueryValue::Bin(vec![]), "Bin(empty)");
    assert_round_trip_identity(&QueryValue::Bin(vec![0u8, 1, 2, 255]), "Bin(bytes)");
}

#[test]
fn round_trip_identity_list() {
    let list = QueryValue::List(vec![
        QueryValue::Null,
        QueryValue::Int(1),
        QueryValue::Str("two".to_string()),
        QueryValue::Bool(true),
        QueryValue::F64(4.0),
    ]);
    assert_round_trip_identity(&list, "List(mixed scalars)");

    // Nested list.
    let nested = QueryValue::List(vec![
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Int(2)]),
        QueryValue::List(vec![QueryValue::Str("a".to_string())]),
    ]);
    assert_round_trip_identity(&nested, "List(nested)");
}

#[test]
fn round_trip_identity_set() {
    let mut s = new_set();
    s.insert(QueryValue::Int(1));
    s.insert(QueryValue::Int(2));
    s.insert(QueryValue::Int(3));
    assert_round_trip_identity(&QueryValue::Set(s), "Set(ints)");

    // Set with mixed-type elements.
    let mut s2 = new_set();
    s2.insert(QueryValue::Str("a".to_string()));
    s2.insert(QueryValue::Str("b".to_string()));
    s2.insert(QueryValue::Int(7));
    assert_round_trip_identity(&QueryValue::Set(s2), "Set(mixed)");
}

#[test]
fn round_trip_identity_map() {
    let mut m = new_map();
    m.insert("name".to_string(), QueryValue::Str("Alice".to_string()));
    m.insert("age".to_string(), QueryValue::Int(30));
    m.insert("active".to_string(), QueryValue::Bool(true));
    m.insert("score".to_string(), QueryValue::F64(9.5));
    assert_round_trip_identity(&QueryValue::Map(m.clone()), "Map(flat)");

    // Unicode keys (the brief explicitly calls out unicode keys as a shape
    // to verify — intern/deintern must round-trip non-ASCII verbatim).
    let mut mu = new_map();
    mu.insert("名前".to_string(), QueryValue::Str("山田".to_string()));
    mu.insert("年齢".to_string(), QueryValue::Int(25));
    assert_round_trip_identity(&QueryValue::Map(mu), "Map(unicode keys)");

    // Nested map (map-of-map, map-of-list, list-of-map).
    let mut inner = new_map();
    inner.insert("x".to_string(), QueryValue::Int(1));
    inner.insert("y".to_string(), QueryValue::Int(2));
    let mut outer = new_map();
    outer.insert("pos".to_string(), QueryValue::Map(inner));
    outer.insert(
        "tags".to_string(),
        QueryValue::List(vec![
            QueryValue::Str("a".to_string()),
            QueryValue::Str("b".to_string()),
        ]),
    );
    assert_round_trip_identity(&QueryValue::Map(outer), "Map(nested)");

    // Empty map.
    let empty: shamir_types::types::common::TMap<String, QueryValue> = new_map();
    assert_round_trip_identity(&QueryValue::Map(empty), "Map(empty)");
}

#[test]
fn round_trip_identity_deeply_nested() {
    // A record-like map combining every scalar, a list, a set, and nested maps.
    let mut rec = new_map();
    rec.insert("id".to_string(), QueryValue::Int(42));
    rec.insert("name".to_string(), QueryValue::Str("Ünïcödé".to_string()));
    rec.insert(
        "balance".to_string(),
        QueryValue::Dec(rust_decimal::Decimal::from_str("1234.5678").unwrap()),
    );
    rec.insert(
        "bigval".to_string(),
        QueryValue::Big(num_bigint::BigInt::from(1_i64) << 80),
    );
    rec.insert(
        "blob".to_string(),
        QueryValue::Bin(vec![0xDE, 0xAD, 0xBE, 0xEF]),
    );
    rec.insert(
        "flags".to_string(),
        QueryValue::List(vec![QueryValue::Bool(true), QueryValue::Bool(false)]),
    );
    let mut tags = new_set();
    tags.insert(QueryValue::Str("alpha".to_string()));
    tags.insert(QueryValue::Str("beta".to_string()));
    rec.insert("tags".to_string(), QueryValue::Set(tags));
    let mut nested = new_map();
    nested.insert("nested_key".to_string(), QueryValue::Null);
    nested.insert("nested_num".to_string(), QueryValue::F64(2.5));
    rec.insert("meta".to_string(), QueryValue::Map(nested));
    assert_round_trip_identity(&QueryValue::Map(rec), "deeply nested record");
}

#[test]
fn round_trip_identity_preserves_map_insertion_order() {
    // IndexMap preserves insertion order through intern→deintern. Verify the
    // key ORDER matches (not just set-equality) so the validator sees the
    // exact same iteration sequence it sees today.
    let mut m = new_map();
    m.insert("zebra".to_string(), QueryValue::Int(1));
    m.insert("apple".to_string(), QueryValue::Int(2));
    m.insert("mango".to_string(), QueryValue::Int(3));
    let qv = QueryValue::Map(m);

    let r = Resolver::new();
    let inner = query_value_to_inner_with(&qv, &r.intern_fn()).unwrap();
    let back = inner_to_query_value_with(&inner, &r.resolve_fn()).unwrap();

    match (back, qv) {
        (Value::Map(got), Value::Map(want)) => {
            let got_keys: Vec<&String> = got.keys().collect();
            let want_keys: Vec<&String> = want.keys().collect();
            assert_eq!(
                got_keys, want_keys,
                "map key insertion order changed through round-trip"
            );
        }
        other => panic!("round-trip changed variant: {:?}", other.0),
    }
}
