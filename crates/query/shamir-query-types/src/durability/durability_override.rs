//! Per-repo durability override — the sparse patch the server config (and,
//! PR-later, per-repo admin ops) applies on top of the server-default
//! durability policy value (owned by the engine's `durability_policy`
//! module).
//!
//! `Default` = every field `None` = inherit the server default untouched.

use core::num::NonZeroU64;

use serde::{Deserialize, Serialize};

use super::{ReplicationServe, RetentionOverflow, WalCommitTier};

mod nonzero_u64_wire {
    use core::num::NonZeroU64;
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<NonZeroU64>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => serializer.serialize_some(&value.get().to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<NonZeroU64>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Decimal(String),
            Number(u64),
        }
        let value = Option::<Wire>::deserialize(deserializer)?;
        let value = match value {
            Some(Wire::Decimal(value)) => value.parse::<u64>().map_err(D::Error::custom)?,
            Some(Wire::Number(value)) => value,
            None => return Ok(None),
        };
        NonZeroU64::new(value)
            .map(Some)
            .ok_or_else(|| D::Error::custom("value must be nonzero"))
    }
}

/// Sparse durability override. Each `Some` field wins over the server
/// default; each `None` field inherits it. Serde-friendly: optional fields
/// are skipped when absent so a JSON/msgpack config object only names the
/// knobs it wants to change.
///
/// `deny_unknown_fields`: a typo'd or not-yet-known knob inside `durability`
/// must be a hard deserialise error, never a silently-ignored default.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurabilityOverride {
    /// Override the WAL commit tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal_commit: Option<WalCommitTier>,
    /// Override the WAL retention budget in bytes. `NonZeroU64`: a zero
    /// budget is meaningless (the whole WAL is instantly over the cap) and
    /// is rejected at deserialise time. Serialize as a decimal string so
    /// values above i64::MAX survive MessagePack consumers with signed-only
    /// integer support; deserialize both this form and legacy integer values.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "nonzero_u64_wire"
    )]
    pub wal_retention_max_bytes: Option<NonZeroU64>,
    /// Override the retention-overflow action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_wal_retention_exceeded: Option<RetentionOverflow>,
    /// Override the replication serve mode (`durability.replication_serve`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replication_serve: Option<ReplicationServe>,
}
