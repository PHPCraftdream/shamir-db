//! Serde round-trip tests for `ConstraintsDto.default` (Phase ②.4b — surface
//! only; stamp-enforcement lands in ②.4c).
//!
//! Invariants under test:
//! - `default: Some(Int(42))` round-trips through msgpack unchanged.
//! - The wire key is exactly `"default"` (the field name — no serde-rename).
//! - `default: Some(...)` is emitted on the wire (NOT skipped).
//! - A legacy `ConstraintsDto` stored WITHOUT `default` deserializes to
//!   `None` (so existing persisted schemas do not change shape on reload).
//! - `default: None` is omitted from the wire (`skip_serializing_if`).

use crate::admin::ConstraintsDto;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

// ── helpers ─────────────────────────────────────────────────────────────────

/// Serialize → decode to `QueryValue` (the wire-level view).
fn wire(c: &ConstraintsDto) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(c).expect("serialize");
    rmp_serde::from_slice(&bytes).expect("QueryValue decode")
}

/// Serialize → deserialize round-trip.
fn round_trip(c: &ConstraintsDto) -> ConstraintsDto {
    let bytes = rmp_serde::to_vec_named(c).expect("serialize");
    rmp_serde::from_slice(&bytes).expect("deserialize")
}

// ── default round-trip + wire shape ─────────────────────────────────────────

/// `default: Some(Int(42))` survives a msgpack round-trip and sits at the
/// top-level `"default"` wire key with the scalar shape intact.
#[test]
fn constraints_default_int_round_trip() {
    let c = ConstraintsDto {
        default: Some(QueryValue::Int(42)),
        ..ConstraintsDto::default()
    };
    let qv = wire(&c);
    assert_eq!(qv.get("default"), Some(&mpack!(42)));
    assert_eq!(round_trip(&c), c);
}

/// `default: Some(Str(...))` round-trips with the string shape intact —
/// proves the field carries any `QueryValue` variant, not just ints.
#[test]
fn constraints_default_str_round_trip() {
    let c = ConstraintsDto {
        default: Some(QueryValue::Str("guest".to_string())),
        ..ConstraintsDto::default()
    };
    assert_eq!(wire(&c).get("default"), Some(&mpack!("guest")));
    assert_eq!(round_trip(&c), c);
}

/// `default: None` is OMITTED from the wire (`skip_serializing_if`).
#[test]
fn constraints_default_absent_when_none() {
    let c = ConstraintsDto::default();
    let qv = wire(&c);
    assert!(
        qv.get("default").is_none(),
        "default must be absent when None, got: {qv:?}"
    );
}

/// A legacy map WITHOUT a `default` key deserializes to `default: None`
/// (existing persisted schemas do not change shape on reload).
#[test]
fn constraints_default_legacy_absent_becomes_none() {
    // Hand-build a minimal wire map with only `required` set (no `default`).
    let mut m = new_map::<String, QueryValue>();
    m.insert("required".to_string(), QueryValue::Bool(true));
    let legacy = QueryValue::Map(m);
    let bytes = rmp_serde::to_vec_named(&legacy).expect("serialize map");
    let c: ConstraintsDto = rmp_serde::from_slice(&bytes).expect("deserialize");
    assert_eq!(c.required, Some(true));
    assert!(
        c.default.is_none(),
        "legacy default must be None, got: {:?}",
        c.default
    );
}
