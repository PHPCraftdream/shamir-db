//! Repository-level DDL operations: create / drop repository.

use serde::{Deserialize, Serialize};

use super::db_ops::is_false;

/// Create a new repository within the current database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateRepoOp {
    pub create_repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<String>,
    /// When `true`, a pre-existing repository with the same name is NOT an
    /// error — the operation returns `{"created": false, "existed": true}`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_not_exists: bool,
}

/// Drop a repository.
///
/// Requires `hmac` over `b"drop_repo\0<db_in_use>\0<repo>"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropRepoOp {
    pub drop_repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
    /// When `true`, all tables inside the repository are removed before
    /// the repository itself is dropped.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cascade: bool,
}
