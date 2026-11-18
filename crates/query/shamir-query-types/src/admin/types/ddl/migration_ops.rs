//! Online table migration DDL operations.
//!
//! Also hosts the node-local online-backup admin ops (R2a step 3a): these
//! target the node's storage roots rather than a specific database, and are
//! superuser-only — authorization is enforced inside `shamir-db`'s admin
//! handler, so `BatchOp::required_access` returns `None` for them (same as
//! every other admin op).

use crate::admin::types::db_ops::is_false;
use serde::{Deserialize, Serialize};

fn default_repo() -> String {
    "main".to_string()
}

/// Start an online table migration to a different storage engine.
///
/// Requires a hex HMAC-SHA256 tag over the canonical v2 operation encoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartMigrationOp {
    pub start_migration: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub dst_repo: String,
    pub dst_engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
}

/// Commit a running migration — performs cutover + swap.
///
/// Requires a hex HMAC-SHA256 tag over the canonical v2 operation encoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitMigrationOp {
    pub commit_migration: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
}

/// Rollback a running (or committed-but-not-dropped) migration.
///
/// Requires a hex HMAC-SHA256 tag over the canonical v2 operation encoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackMigrationOp {
    pub rollback_migration: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
}

/// Query the status of a migration by ID, or list all active migrations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationStatusOp {
    pub migration_status: String,
}

// ------------------------------------------------------------------------
// Node-local online-backup admin ops (R2a step 3a).
//
// Pure wire/serde DTOs like the rest of this module: the wire discriminator
// is a presence-only boolean flag (mirroring `ListPublicationsOp`), sitting
// flat in the op map next to the op's own fields.
// ------------------------------------------------------------------------

/// Start an online backup into the operator-supplied destination root path.
///
/// Superuser-only, node-local admin operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartBackupOp {
    /// Discriminator flag — presence-only, conventionally `true`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub start_backup: bool,
    /// Destination root path the backup is written into.
    pub to: String,
}

/// Query the status of the running (or last finished) online backup.
///
/// Superuser-only, node-local admin operation; read-only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupStatusOp {
    /// Discriminator flag — presence-only, conventionally `true`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub backup_status: bool,
}

/// Cancel the running online backup.
///
/// Superuser-only, node-local admin operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelBackupOp {
    /// Discriminator flag — presence-only, conventionally `true`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cancel_backup: bool,
}
