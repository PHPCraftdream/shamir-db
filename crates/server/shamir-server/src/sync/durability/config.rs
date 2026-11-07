use serde::Deserialize;
use shamir_db::engine::repo::durability_policy::{
    effective_wal_segment_bytes, resolve, DurabilityPolicy, DEFAULT_WAL_RETENTION_MAX_BYTES,
};
use shamir_db::DbError;
use shamir_query_types::{ReplicationServe, RetentionOverflow, WalCommitTier};

use crate::config::ConfigError;

/// Server-default WAL policy. Unknown keys and enum spellings fail parsing.
/// Retention is parsed/validated now; enforcement is planned for PR2.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DurabilityConfig {
    pub wal_commit: WalCommitTier,
    pub wal_retention_max_bytes: u64,
    pub on_wal_retention_exceeded: RetentionOverflow,
    pub replication_serve: ReplicationServe,
}

impl Default for DurabilityConfig {
    fn default() -> Self {
        Self {
            wal_commit: WalCommitTier::Buffered,
            wal_retention_max_bytes: DEFAULT_WAL_RETENTION_MAX_BYTES,
            on_wal_retention_exceeded: RetentionOverflow::RefuseWrites,
            replication_serve: ReplicationServe::Durable,
        }
    }
}

impl DurabilityConfig {
    /// Resolve against the same env-aware segment size used by WAL creation.
    pub fn resolve_policy(&self) -> Result<DurabilityPolicy, ConfigError> {
        self.resolve_for_segment(effective_wal_segment_bytes())
    }

    pub(crate) fn resolve_for_segment(
        &self,
        effective_segment_bytes: u64,
    ) -> Result<DurabilityPolicy, ConfigError> {
        let policy = DurabilityPolicy::new(
            self.wal_commit,
            self.wal_retention_max_bytes,
            self.on_wal_retention_exceeded,
            self.replication_serve,
        );
        resolve(&policy, None, effective_segment_bytes).map_err(|e| match e {
            DbError::Internal(reason) => ConfigError::Validation(reason),
            other => ConfigError::Validation(other.to_string()),
        })
    }

    /// Boot warning only: accepting DeclareGap does not implement eviction.
    pub(crate) fn warn_unimplemented_retention(&self) {
        if self.on_wal_retention_exceeded == RetentionOverflow::DeclareGap {
            tracing::warn!(
                "durability.on_wal_retention_exceeded=declare_gap configured, but WAL retention enforcement is NOT implemented until PR2"
            );
        }
    }
}
