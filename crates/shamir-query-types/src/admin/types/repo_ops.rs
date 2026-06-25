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
    /// When `true`, dropping a non-existent repository is a silent no-op
    /// returning `{"existed": false}` instead of an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_exists: bool,
}

/// Rename a repository inside the current database, preserving all of its
/// tables, their data, indexes, and catalogue metadata.
///
/// The repository's record in the catalogue is re-keyed: the old
/// `(db, from)` row is removed and a new `(db, to)` row is written with
/// the same `engine`/`path` and the same `ResourceMeta` (owner/group/mode)
/// as the original. Every child table's catalogue row is likewise re-keyed
/// from `(db, from, table)` to `(db, to, table)`. In-memory, the
/// `RepoInstance` is moved to the new key in `DbInstance::repos` and its
/// internal `name` field is updated — **no per-table store copy happens**
/// because table stores are keyed only by table name inside the repo (the
/// repo name is NOT part of the physical store namespace).
///
/// Guards (refuse with a typed error instead of leaving dangling state):
/// - The source repo must exist.
/// - The destination must NOT exist.
///
/// ```text
/// { "rename_repo": "old_name", "to": "new_name" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameRepoOp {
    pub rename_repo: String,
    pub to: String,
}
