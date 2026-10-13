//! Wire-shape tests for the durability DTOs (snake_case serde strings).
//!
//! This crate serialises DTOs with msgpack (`rmp-serde`, named mode); the
//! same serde derives serve the server's config layer.

use core::num::NonZeroU64;

use super::super::{DurabilityOverride, ReplicationServe, RetentionOverflow, WalCommitTier};

#[test]
fn wal_commit_tier_wire_names() {
    // MessagePack fixstr marker is 0xa0 | byte length (all names < 32 bytes).
    for (tier, golden) in [
        (WalCommitTier::Buffered, b"\xa8buffered".as_slice()),
        (WalCommitTier::Synced, b"\xa6synced".as_slice()),
    ] {
        let bytes = rmp_serde::to_vec_named(&tier).unwrap();
        assert_eq!(bytes.as_slice(), golden);
        let back: WalCommitTier = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back, tier);
    }
}

#[test]
fn retention_overflow_wire_names() {
    for (action, golden) in [
        (
            RetentionOverflow::RefuseWrites,
            b"\xadrefuse_writes".as_slice(),
        ),
        (RetentionOverflow::DeclareGap, b"\xabdeclare_gap".as_slice()),
    ] {
        let bytes = rmp_serde::to_vec_named(&action).unwrap();
        assert_eq!(bytes.as_slice(), golden);
        let back: RetentionOverflow = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back, action);
    }
}

#[test]
fn override_is_sparse_on_the_wire() {
    // Default: all None — serialises to an empty map.
    let empty = rmp_serde::to_vec_named(&DurabilityOverride::default()).unwrap();
    assert_eq!(empty, b"\x80", "skip_serializing_if must emit an empty map");
    let back: DurabilityOverride = rmp_serde::from_slice(&empty).unwrap();
    assert_eq!(back, DurabilityOverride::default());

    // Only the named knobs appear; unnamed knobs inherit (absent).
    let override_json = rmp_serde::to_vec_named(&DurabilityOverride {
        wal_commit: Some(WalCommitTier::Synced),
        wal_retention_max_bytes: Some(NonZeroU64::new(1024).unwrap()),
        on_wal_retention_exceeded: None,
        replication_serve: None,
    })
    .unwrap();
    let back: DurabilityOverride = rmp_serde::from_slice(&override_json).unwrap();
    assert_eq!(back.wal_commit, Some(WalCommitTier::Synced));
    assert_eq!(
        back.wal_retention_max_bytes,
        Some(NonZeroU64::new(1024).unwrap())
    );
    assert_eq!(back.on_wal_retention_exceeded, None);
    assert_eq!(back.replication_serve, None);
}

#[test]
fn replication_serve_wire_names() {
    for (mode, golden) in [
        (ReplicationServe::Durable, b"\xa7durable".as_slice()),
        (
            ReplicationServe::CompletedReseed,
            b"\xb0completed_reseed".as_slice(),
        ),
    ] {
        let bytes = rmp_serde::to_vec_named(&mode).unwrap();
        assert_eq!(bytes.as_slice(), golden);
        let back: ReplicationServe = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back, mode);
    }
    // Default = Durable (the only mode PR1 accepts).
    assert_eq!(ReplicationServe::default(), ReplicationServe::Durable);
}
