//! Online table migration DDL operations.

use serde::{Deserialize, Serialize};

fn default_repo() -> String {
    "main".to_string()
}

/// Start an online table migration to a different storage engine.
///
/// Requires `hmac` over
/// `b"start_migration\0<db>\0<src_repo>\0<table>\0<dst_repo>\0<dst_engine>"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
/// Requires `hmac` over `b"commit_migration\0<db>\0<migration_id>"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitMigrationOp {
    pub commit_migration: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
}

/// Rollback a running (or committed-but-not-dropped) migration.
///
/// Requires `hmac` over `b"rollback_migration\0<db>\0<migration_id>"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RollbackMigrationOp {
    pub rollback_migration: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
}

/// Query the status of a migration by ID, or list all active migrations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationStatusOp {
    pub migration_status: String,
}
