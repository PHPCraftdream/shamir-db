//! Per-repo durability override — the sparse patch the server config (and,
//! PR-later, per-repo admin ops) applies on top of the server-default
//! durability policy value (owned by the engine's `durability_policy`
//! module).
//!
//! `Default` = every field `None` = inherit the server default untouched.

use core::num::NonZeroU64;

use serde::{Deserialize, Serialize};

use super::{ReplicationServe, RetentionOverflow, WalCommitTier};

/// Sparse durability override. Each `Some` field wins over the server
/// default; each `None` field inherits it. Serde-friendly: optional fields
/// are skipped when absent so a JSON/msgpack config object only names the
/// knobs it wants to change.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct DurabilityOverride {
    /// Override the WAL commit tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal_commit: Option<WalCommitTier>,
    /// Override the WAL retention budget in bytes. `NonZeroU64`: a zero
    /// budget is meaningless (the whole WAL is instantly over the cap) and
    /// is rejected at deserialise time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal_retention_max_bytes: Option<NonZeroU64>,
    /// Override the retention-overflow action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_wal_retention_exceeded: Option<RetentionOverflow>,
    /// Override the replication serve mode (`durability.replication_serve`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replication_serve: Option<ReplicationServe>,
}
