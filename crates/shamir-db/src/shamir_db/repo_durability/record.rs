//! Encode/decode the sparse per-repo durability override inside the
//! system-store `repositories` catalogue record.
//!
//! Encoding rules (S5c-X PR1b part 2):
//! - the override lives under the record key `durability`; a legacy record
//!   without that key decodes as "no override" (`Ok(None)`), never as an
//!   error;
//! - enum values are stored as their serde snake_case spellings
//!   (`"buffered"`, `"synced"`, `"refuse_writes"`, `"declare_gap"`,
//!   `"durable"`, `"completed_reseed"`);
//! - `wal_retention_max_bytes` uses the native lossless `QueryValue::from(u64)`
//!   encoding (including `Big` above `i64::MAX`), rather than narrowing to i64;
//!   decode also accepts the legacy decimal-string spelling;
//! - absent keys decode as `None` fields, matching the wire DTO's
//!   `skip_serializing_if = "Option::is_none"` shape;
//! - a corrupt `durability` value (non-object, unknown key, unknown enum
//!   spelling, zero retention) is a LOUD typed error — never silence.

use core::num::NonZeroU64;

use shamir_query_types::durability::{
    DurabilityOverride, ReplicationServe, RetentionOverflow, WalCommitTier,
};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::DbError;

/// Encode an override into the `durability` record value.
pub(crate) fn durability_to_record(over: &DurabilityOverride) -> QueryValue {
    let mut m = new_map();
    if let Some(wal_commit) = over.wal_commit {
        m.insert(
            "wal_commit".to_string(),
            QueryValue::Str(wal_commit_tier_to_str(wal_commit).to_string()),
        );
    }
    if let Some(bytes) = over.wal_retention_max_bytes {
        m.insert(
            "wal_retention_max_bytes".to_string(),
            QueryValue::from(bytes.get()),
        );
    }
    if let Some(action) = over.on_wal_retention_exceeded {
        m.insert(
            "on_wal_retention_exceeded".to_string(),
            QueryValue::Str(retention_overflow_to_str(action).to_string()),
        );
    }
    if let Some(serve) = over.replication_serve {
        m.insert(
            "replication_serve".to_string(),
            QueryValue::Str(replication_serve_to_str(serve).to_string()),
        );
    }
    QueryValue::Map(m)
}

/// Decode the `durability` key of a repository catalogue record.
///
/// `Ok(None)` when the key is absent (legacy record) or explicit `Null`.
/// Any other non-object value, unknown key, unknown enum spelling, or a
/// zero `wal_retention_max_bytes` is a typed error.
pub(crate) fn durability_from_record(
    record: &QueryValue,
) -> Result<Option<DurabilityOverride>, DbError> {
    let value = match record.get("durability") {
        None | Some(QueryValue::Null) => return Ok(None),
        Some(v) => v,
    };
    let m = value.as_object().ok_or_else(|| {
        DbError::Validation(format!(
            "repository record field 'durability' must be an object, got {:?}",
            value
        ))
    })?;
    let mut over = DurabilityOverride::default();
    for (key, v) in m.iter() {
        match key.as_str() {
            "wal_commit" => {
                over.wal_commit = Some(wal_commit_tier_from_str(str_field(key, v)?)?);
            }
            "wal_retention_max_bytes" => {
                let raw = u64_field(key, v)?;
                over.wal_retention_max_bytes = Some(NonZeroU64::new(raw).ok_or_else(|| {
                    DbError::Validation(format!(
                        "repository record field 'durability.{}' must be non-zero",
                        key
                    ))
                })?);
            }
            "on_wal_retention_exceeded" => {
                over.on_wal_retention_exceeded =
                    Some(retention_overflow_from_str(str_field(key, v)?)?);
            }
            "replication_serve" => {
                over.replication_serve = Some(replication_serve_from_str(str_field(key, v)?)?);
            }
            other => {
                return Err(DbError::Validation(format!(
                    "repository record field 'durability.{}' is unknown",
                    other
                )));
            }
        }
    }
    Ok(Some(over))
}

/// ALTER patch semantics: fields `Some` in `patch` win, `None` fields keep
/// the stored `base` value.
pub(crate) fn merge_override(
    base: &DurabilityOverride,
    patch: &DurabilityOverride,
) -> DurabilityOverride {
    DurabilityOverride {
        wal_commit: patch.wal_commit.or(base.wal_commit),
        wal_retention_max_bytes: patch
            .wal_retention_max_bytes
            .or(base.wal_retention_max_bytes),
        on_wal_retention_exceeded: patch
            .on_wal_retention_exceeded
            .or(base.on_wal_retention_exceeded),
        replication_serve: patch.replication_serve.or(base.replication_serve),
    }
}

// ── field readers ───────────────────────────────────────────────────────

fn str_field<'a>(key: &str, v: &'a QueryValue) -> Result<&'a str, DbError> {
    v.as_str().ok_or_else(|| {
        DbError::Validation(format!(
            "repository record field 'durability.{}' must be a string, got {:?}",
            key, v
        ))
    })
}

/// Accepts canonical `Int` / `Big`, plus legacy decimal string spellings.
fn u64_field(key: &str, v: &QueryValue) -> Result<u64, DbError> {
    let parsed = match v {
        QueryValue::Str(s) => s.parse::<u64>().ok(),
        QueryValue::Int(i) if *i >= 0 => Some(*i as u64),
        QueryValue::Big(b) => b.to_string().parse::<u64>().ok(),
        _ => None,
    };
    parsed.ok_or_else(|| {
        DbError::Validation(format!(
            "repository record field 'durability.{}' must be a u64, got {:?}",
            key, v
        ))
    })
}

// ── enum ↔ snake_case spellings (same as serde rename_all) ─────────────

fn wal_commit_tier_to_str(tier: WalCommitTier) -> &'static str {
    match tier {
        WalCommitTier::Buffered => "buffered",
        WalCommitTier::Synced => "synced",
    }
}

fn wal_commit_tier_from_str(s: &str) -> Result<WalCommitTier, DbError> {
    match s {
        "buffered" => Ok(WalCommitTier::Buffered),
        "synced" => Ok(WalCommitTier::Synced),
        other => Err(DbError::Validation(format!(
            "repository record field 'durability.wal_commit' has unknown value '{}'",
            other
        ))),
    }
}

fn retention_overflow_to_str(action: RetentionOverflow) -> &'static str {
    match action {
        RetentionOverflow::RefuseWrites => "refuse_writes",
        RetentionOverflow::DeclareGap => "declare_gap",
    }
}

fn retention_overflow_from_str(s: &str) -> Result<RetentionOverflow, DbError> {
    match s {
        "refuse_writes" => Ok(RetentionOverflow::RefuseWrites),
        "declare_gap" => Ok(RetentionOverflow::DeclareGap),
        other => Err(DbError::Validation(format!(
            "repository record field 'durability.on_wal_retention_exceeded' has unknown value '{}'",
            other
        ))),
    }
}

fn replication_serve_to_str(serve: ReplicationServe) -> &'static str {
    match serve {
        ReplicationServe::Durable => "durable",
        ReplicationServe::CompletedReseed => "completed_reseed",
    }
}

fn replication_serve_from_str(s: &str) -> Result<ReplicationServe, DbError> {
    match s {
        "durable" => Ok(ReplicationServe::Durable),
        "completed_reseed" => Ok(ReplicationServe::CompletedReseed),
        other => Err(DbError::Validation(format!(
            "repository record field 'durability.replication_serve' has unknown value '{}'",
            other
        ))),
    }
}
