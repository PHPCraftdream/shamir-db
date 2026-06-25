//! Serde round-trip tests for `FkAction` and the `ForeignKeyDto.on_delete` /
//! `on_update` fields (Phase D.0 / ②.2a).
//!
//! The critical invariant under test: a LEGACY `ForeignKeyDto` stored WITHOUT
//! an `on_delete` / `on_update` field MUST deserialize to `FkAction::NoAction`
//! so that existing persisted schemas do not change behavior on reload.

use crate::admin::{FkAction, ForeignKeyDto};
use shamir_types::mpack;

// ── helpers ─────────────────────────────────────────────────────────────────

fn ser(v: &ForeignKeyDto) -> shamir_types::types::value::QueryValue {
    let bytes = rmp_serde::to_vec_named(v).expect("serialize");
    rmp_serde::from_slice(&bytes).expect("QueryValue decode")
}

fn round_trip(v: &ForeignKeyDto) -> ForeignKeyDto {
    let bytes = rmp_serde::to_vec_named(v).expect("serialize");
    rmp_serde::from_slice(&bytes).expect("deserialize")
}

// ── wire shape per variant ──────────────────────────────────────────────────

/// `NoAction` is the serde default AND must be omitted from the wire
/// (`skip_serializing_if = "FkAction::is_no_action"`).
#[test]
fn fk_no_action_omitted_from_wire() {
    let fk = ForeignKeyDto {
        ref_table: "parent".to_string(),
        ref_field: "id".to_string(),
        on_delete: FkAction::NoAction,
        on_update: FkAction::NoAction,
    };
    let qv = ser(&fk);
    assert_eq!(qv.get("ref_table"), Some(&mpack!("parent")));
    assert_eq!(qv.get("ref_field"), Some(&mpack!("id")));
    assert!(
        qv.get("on_delete").is_none(),
        "NoAction must be omitted from the wire, got: {qv:?}"
    );
    assert!(
        qv.get("on_update").is_none(),
        "NoAction on_update must be omitted from the wire, got: {qv:?}"
    );
    assert_eq!(round_trip(&fk), fk);
}

#[test]
fn fk_restrict_wire_shape() {
    let fk = ForeignKeyDto {
        ref_table: "parent".to_string(),
        ref_field: "id".to_string(),
        on_delete: FkAction::Restrict,
        on_update: FkAction::NoAction,
    };
    let qv = ser(&fk);
    assert_eq!(qv.get("on_delete"), Some(&mpack!("restrict")));
    assert_eq!(round_trip(&fk), fk);
}

#[test]
fn fk_cascade_wire_shape() {
    let fk = ForeignKeyDto {
        ref_table: "parent".to_string(),
        ref_field: "id".to_string(),
        on_delete: FkAction::Cascade,
        on_update: FkAction::NoAction,
    };
    let qv = ser(&fk);
    assert_eq!(qv.get("on_delete"), Some(&mpack!("cascade")));
    assert_eq!(round_trip(&fk), fk);
}

#[test]
fn fk_set_null_wire_shape() {
    let fk = ForeignKeyDto {
        ref_table: "parent".to_string(),
        ref_field: "id".to_string(),
        on_delete: FkAction::SetNull,
        on_update: FkAction::NoAction,
    };
    let qv = ser(&fk);
    assert_eq!(qv.get("on_delete"), Some(&mpack!("set_null")));
    assert_eq!(round_trip(&fk), fk);
}

// ── Phase ②.2a — on_update wire shape ───────────────────────────────────────

/// `on_update: Cascade` survives a serde round-trip and appears on the wire.
#[test]
fn fk_on_update_cascade_round_trip() {
    let fk = ForeignKeyDto {
        ref_table: "parent".to_string(),
        ref_field: "id".to_string(),
        on_delete: FkAction::NoAction,
        on_update: FkAction::Cascade,
    };
    let qv = ser(&fk);
    assert_eq!(qv.get("on_update"), Some(&mpack!("cascade")));
    // on_delete is NoAction → omitted.
    assert!(qv.get("on_delete").is_none());
    assert_eq!(round_trip(&fk), fk);
}

/// Both actions set (on_delete=SetNull, on_update=Cascade) round-trip together.
#[test]
fn fk_both_actions_round_trip() {
    let fk = ForeignKeyDto {
        ref_table: "parent".to_string(),
        ref_field: "id".to_string(),
        on_delete: FkAction::SetNull,
        on_update: FkAction::Cascade,
    };
    let qv = ser(&fk);
    assert_eq!(qv.get("on_delete"), Some(&mpack!("set_null")));
    assert_eq!(qv.get("on_update"), Some(&mpack!("cascade")));
    assert_eq!(round_trip(&fk), fk);
}

// ── backward-compat: legacy bytes WITHOUT on_delete ────────────────────────

/// The critical backward-compat invariant: a `ForeignKeyDto` stored before
/// Phase D.0 / ②.2a (no `on_delete` / `on_update` keys in the persisted
/// bytes) MUST deserialize both actions to `FkAction::NoAction`. This
/// guarantees reload does not change behavior for existing schemas.
#[test]
fn fk_legacy_bytes_without_on_delete_default_to_no_action() {
    // Hand-build the legacy msgpack map: { "ref_table": "parent", "ref_field": "id" }
    // with NO on_delete / on_update keys.
    let legacy = mpack!({
        "ref_table": "parent",
        "ref_field": "id",
    });
    let bytes = rmp_serde::to_vec_named(&legacy).expect("serialize legacy");
    let fk: ForeignKeyDto = rmp_serde::from_slice(&bytes).expect("deserialize legacy");
    assert_eq!(fk.ref_table, "parent");
    assert_eq!(fk.ref_field, "id");
    assert_eq!(
        fk.on_delete,
        FkAction::NoAction,
        "legacy FK without on_delete must default to NoAction"
    );
    assert_eq!(
        fk.on_update,
        FkAction::NoAction,
        "legacy FK without on_update must default to NoAction"
    );
}

/// `FkAction::default() == NoAction` — the conservative wire default.
#[test]
fn fk_action_default_is_no_action() {
    assert_eq!(FkAction::default(), FkAction::NoAction);
}

/// `is_no_action` predicate is correct for all variants.
#[test]
fn fk_action_is_no_action_predicate() {
    assert!(FkAction::NoAction.is_no_action());
    assert!(!FkAction::Restrict.is_no_action());
    assert!(!FkAction::Cascade.is_no_action());
    assert!(!FkAction::SetNull.is_no_action());
}
