//! Wire-shape tests for the per-repo `DurabilityOverride` as embedded in
//! admin repo ops (`CreateRepoOp.durability` / `AlterRepoOp.durability`).
//!
//! The override rides inside a `BatchOp` map decoded through
//! `rmp_serde::from_slice` (the `qv_to` path in `batch_op.rs`), so serde
//! attributes — `deny_unknown_fields`, `NonZeroU64`, enum variants — apply
//! at deserialise time. These tests pin that no unknown key is silently
//! ignored and no bad value defaults through.

use std::collections::BTreeMap;

use super::super::{DurabilityOverride, ReplicationServe, RetentionOverflow, WalCommitTier};

/// All four knobs set must survive a msgpack round trip unchanged.
#[test]
fn override_full_round_trip() {
    let original = DurabilityOverride {
        wal_commit: Some(WalCommitTier::Synced),
        wal_retention_max_bytes: Some(std::num::NonZeroU64::new(4096).unwrap()),
        on_wal_retention_exceeded: Some(RetentionOverflow::DeclareGap),
        replication_serve: Some(ReplicationServe::Durable),
    };
    let bytes = rmp_serde::to_vec_named(&original).unwrap();
    let back: DurabilityOverride = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, original);
}

/// `deny_unknown_fields`: an unknown key inside the override map must be a
/// hard deserialise error naming the field — never a silent default.
#[test]
fn override_unknown_key_is_an_error() {
    let bogus: BTreeMap<&str, &str> = BTreeMap::from([
        ("wal_commit", "synced"),
        ("wal_commit_tier", "synced"), // plausible typo — must NOT be accepted
    ]);
    let bytes = rmp_serde::to_vec_named(&bogus).unwrap();
    let err = rmp_serde::from_slice::<DurabilityOverride>(&bytes)
        .expect_err("unknown key inside durability must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown field"),
        "error must name the unknown field, got: {msg}"
    );
    assert!(
        msg.contains("wal_commit_tier"),
        "error must name the offending key, got: {msg}"
    );
}

/// A bad enum string inside the override must be a hard error naming the
/// variant — not a silent fall-through to the default tier.
#[test]
fn override_bad_enum_string_is_an_error() {
    let bogus: BTreeMap<&str, &str> = BTreeMap::from([("wal_commit", "eventual")]);
    let bytes = rmp_serde::to_vec_named(&bogus).unwrap();
    let err =
        rmp_serde::from_slice::<DurabilityOverride>(&bytes).expect_err("bad enum string must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown variant"),
        "error must name the unknown variant, got: {msg}"
    );
    assert!(
        msg.contains("eventual"),
        "error must name the offending string, got: {msg}"
    );
}

/// `wal_retention_max_bytes` is `NonZeroU64`: a zero budget is meaningless
/// (the whole WAL is instantly over the cap) and must be rejected at
/// deserialise time.
#[test]
fn override_zero_retention_budget_is_an_error() {
    let bogus: BTreeMap<&str, u64> = BTreeMap::from([("wal_retention_max_bytes", 0)]);
    let bytes = rmp_serde::to_vec_named(&bogus).unwrap();
    let err = rmp_serde::from_slice::<DurabilityOverride>(&bytes)
        .expect_err("zero retention budget must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("invalid value"),
        "error must be a value-domain rejection, got: {msg}"
    );
}
