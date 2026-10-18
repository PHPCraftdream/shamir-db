//! Serde round-trip tests for `ConstraintsDto.default` (③.2c: extended from
//! Phase ②.4b literal-only to expression).
//!
//! Invariants under test:
//! - `default: Some(FilterValue::Int(42))` round-trips through msgpack unchanged.
//! - Literal FilterValue encodes identically to the equivalent `QueryValue`
//!   wire shape (serde backward compatibility — ②.4b schemas round-trip).
//! - Expression FilterValue (`$fn`) also round-trips through msgpack.
//! - The wire key is exactly `"default"` (the field name — no serde-rename).
//! - `default: Some(...)` is emitted on the wire (NOT skipped).
//! - A legacy `ConstraintsDto` stored WITHOUT `default` deserializes to
//!   `None` (so existing persisted schemas do not change shape on reload).
//! - `default: None` is omitted from the wire (`skip_serializing_if`).

use crate::admin::ConstraintsDto;
use crate::filter::{FilterValue, FnCall};
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

// ── default round-trip + wire shape (literal defaults, ②.4b regression) ─────

/// `default: Some(FilterValue::Int(42))` survives a msgpack round-trip and
/// sits at the top-level `"default"` wire key with the scalar shape intact.
/// Wire shape must match the `QueryValue::Int(42)` legacy shape (backward compat).
#[test]
fn constraints_default_int_round_trip() {
    let c = ConstraintsDto {
        default: Some(FilterValue::Int(42)),
        ..ConstraintsDto::default()
    };
    // Wire shape must equal QueryValue::Int(42) (serde-untagged-compatible).
    let qv = wire(&c);
    assert_eq!(qv.get("default"), Some(&mpack!(42)));
    assert_eq!(round_trip(&c), c);
}

/// `default: Some(FilterValue::String(...))` round-trips with the string shape
/// intact — proves the field carries any literal FilterValue variant.
#[test]
fn constraints_default_str_round_trip() {
    let c = ConstraintsDto {
        default: Some(FilterValue::String("guest".to_string())),
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

// ── expression defaults (③.2c) ───────────────────────────────────────────────

/// `default: Some(FilterValue::FnCall(...))` round-trips through msgpack,
/// proving expression defaults can be stored and recovered from the catalogue.
#[test]
fn constraints_default_fn_call_expression_round_trip() {
    let c = ConstraintsDto {
        default: Some(FilterValue::FnCall {
            call: FnCall::simple("strings/upper"),
        }),
        ..ConstraintsDto::default()
    };
    let rt = round_trip(&c);
    assert_eq!(
        rt.default, c.default,
        "expression default must round-trip unchanged"
    );
    // Wire shape must contain the `$fn` key at `"default"`.
    let qv = wire(&c);
    let default_qv = qv.get("default").expect("default must be present");
    // The $fn key must survive as a map key at the wire level.
    assert!(
        default_qv.get("$fn").is_some(),
        "expression default wire shape must contain '$fn', got: {default_qv:?}"
    );
}
