//! Backend-independent durability-policy tests: `resolve` validation and
//! the Mem-WAL-sink canaries. These need no `fjall` feature (the repo
//! fixtures here are `mem_repo` or pure policy functions).

use shamir_query_types::batch::VisibilityWait;
use shamir_query_types::{DurabilityOverride, ReplicationServe, RetentionOverflow};
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

use crate::repo::durability_policy::{
    resolve, DurabilityPolicy, WalCommitTier, DEFAULT_WAL_RETENTION_MAX_BYTES,
};

use super::super::helpers;

/// PR8 chunk 1: on an ephemeral backend `set_durability` SUCCEEDS (the only
/// honest serve mode there), and an explicitly requested `synced` commit
/// tier is silently downgraded to `buffered` via `for_wal_backend` — the
/// requested tier stays visible in `wal_commit()` while
/// `effective_wal_commit()` reports the downgrade.
#[test]
fn public_policy_setter_rejects_invalid_modes_without_changing_clones() {
    // `synced` alone is VALID again (silently downgraded on ephemeral
    // backends); only the retention-under-floor entry stays invalid.
    let repo = helpers::mem_repo("setter-validation");
    let clone = repo.clone();
    let before = **clone.durability_policy();
    let invalid = DurabilityPolicy::new(
        WalCommitTier::Synced,
        1,
        RetentionOverflow::RefuseWrites,
        ReplicationServe::Durable,
    );
    assert!(repo.set_durability(invalid).is_err());
    assert_eq!(**clone.durability_policy(), before);
}

/// Amendment B2 (3): `resolve` — defaults pass; each override field wins;
/// a retention budget under 2 × segment size is rejected with a message
/// naming the key, the value and the minimum.
#[test]
fn resolve_defaults_and_override_win_and_floor() {
    let default = DurabilityPolicy::default();
    let segment = 64 * 1024 * 1024_u64;

    // (a) PR1 shape: no override → server default passes the floor.
    let resolved = resolve(&default, None, segment).unwrap();
    assert_eq!(resolved, default);

    // (b) each Some(override) field wins.
    let over = DurabilityOverride {
        wal_commit: Some(WalCommitTier::Synced),
        wal_retention_max_bytes: Some(core::num::NonZeroU64::new(4 * segment).unwrap()),
        on_wal_retention_exceeded: Some(RetentionOverflow::DeclareGap),
        replication_serve: None,
    };
    let resolved = resolve(&default, Some(&over), segment).unwrap();
    assert_eq!(resolved.wal_commit(), WalCommitTier::Synced);
    assert_eq!(resolved.wal_retention_max_bytes(), 4 * segment);
    assert_eq!(
        resolved.on_wal_retention_exceeded(),
        RetentionOverflow::DeclareGap
    );

    // (c) below the 2 × segment floor → Err naming key, value, minimum.
    let tiny = DurabilityOverride {
        wal_retention_max_bytes: Some(core::num::NonZeroU64::new(3).unwrap()),
        ..Default::default()
    };
    let err = resolve(&default, Some(&tiny), segment)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("wal_retention_max_bytes")
            && err.contains('3')
            && err.contains(&(2 * segment).to_string()),
        "error must name the key, configured value and minimum: {err}"
    );

    // (d) None fields inherit — a tier-only override keeps the budget.
    let tier_only = DurabilityOverride {
        wal_commit: Some(WalCommitTier::Buffered),
        ..Default::default()
    };
    let synced_default = DurabilityPolicy::new(
        WalCommitTier::Synced,
        4 * segment,
        RetentionOverflow::DeclareGap,
        ReplicationServe::Durable,
    );
    let resolved = resolve(&synced_default, Some(&tier_only), segment).unwrap();
    assert_eq!(resolved.wal_commit(), WalCommitTier::Buffered);
    assert_eq!(resolved.wal_retention_max_bytes(), 4 * segment);
    assert_eq!(
        resolved.on_wal_retention_exceeded(),
        RetentionOverflow::DeclareGap
    );
}

/// PR8 chunk 1: `replication_serve = completed_reseed` RESOLVES — through
/// a server default AND through a per-repo override — and the retention
/// floor still fires for a `completed_reseed` policy under the minimum.
#[test]
fn completed_reseed_resolves_through_server_default_and_override() {
    let durable_default = DurabilityPolicy::default();
    let segment = 64 * 1024 * 1024_u64;

    // (a) via the server default itself.
    let default_with_completed_reseed = DurabilityPolicy::new(
        WalCommitTier::Buffered,
        DEFAULT_WAL_RETENTION_MAX_BYTES,
        RetentionOverflow::RefuseWrites,
        ReplicationServe::CompletedReseed,
    );
    let resolved = resolve(&default_with_completed_reseed, None, segment).unwrap();
    assert_eq!(
        resolved.replication_serve(),
        ReplicationServe::CompletedReseed
    );
    assert_eq!(
        resolved.wal_retention_max_bytes(),
        DEFAULT_WAL_RETENTION_MAX_BYTES
    );

    // (b) via the override over a durable default.
    let over = DurabilityOverride {
        replication_serve: Some(ReplicationServe::CompletedReseed),
        ..Default::default()
    };
    let resolved = resolve(&durable_default, Some(&over), segment).unwrap();
    assert_eq!(
        resolved.replication_serve(),
        ReplicationServe::CompletedReseed
    );
    assert_eq!(resolved, default_with_completed_reseed);

    // (c) the retention floor still fires for a completed_reseed policy.
    let tiny = DurabilityPolicy::new(
        WalCommitTier::Buffered,
        3,
        RetentionOverflow::RefuseWrites,
        ReplicationServe::CompletedReseed,
    );
    let err = resolve(&tiny, None, segment).unwrap_err().to_string();
    assert!(
        err.contains("wal_retention_max_bytes")
            && err.contains('3')
            && err.contains(&(2 * segment).to_string()),
        "retention floor error must name key, value and minimum: {err}"
    );
}

/// Hang canary, in-memory variant: the same single buffered commit through the
/// Mem WAL sink on the default current-thread runtime. The commit/ack code is
/// shared across backends (ticket completion, completion-watermark advance,
/// visibility observation), so this pins the backend-independent half.
#[tokio::test]
async fn buffered_commit_completes_on_default_runtime_mem_wal() {
    let repo = helpers::mem_repo("canary_buffered_commit_mem");
    let table = repo.get_table("items").await.unwrap();
    let (mut tx, pin) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str("buffered".into()), Some(&mut tx))
        .await
        .unwrap();
    let info = repo
        .commit_tx_with_options(tx, VisibilityWait::Strict)
        .await
        .unwrap();
    drop(pin);
    assert_eq!(
        info.visibility,
        shamir_query_types::batch::VisibilityState::Visible
    );
}
