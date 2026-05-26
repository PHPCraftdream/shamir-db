//! Admin (DDL) operation types.

use serde::{Deserialize, Serialize};

/// Create a new database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateDbOp {
    pub create_db: String,
}

/// Drop a database.
///
/// Requires an `hmac` field — hex-encoded HMAC-SHA256 tag over
/// `b"drop_db\0<db_name>"` keyed by the session HMAC key
/// (`SHA256("shamir-db hmac key v1\0" || session_id)`). Missing /
/// wrong tag → request rejected with `hmac_required` /
/// `hmac_mismatch`. See `query_buffer_config.rs` design notes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropDbOp {
    pub drop_db: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
}

/// Create a new repository within the current database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateRepoOp {
    pub create_repo: String,
    #[serde(default = "default_engine")]
    pub engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<String>,
}

fn default_engine() -> String {
    "in_memory".to_string()
}

/// Drop a repository.
///
/// Requires `hmac` over `b"drop_repo\0<db_in_use>\0<repo>"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropRepoOp {
    pub drop_repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
}

/// Create a table in a repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTableOp {
    pub create_table: String,
    #[serde(default = "default_repo")]
    pub repo: String,
}

fn default_repo() -> String {
    "main".to_string()
}

/// Drop a table.
///
/// Requires `hmac` over `b"drop_table\0<db_in_use>\0<repo>\0<table>"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropTableOp {
    pub drop_table: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
}

/// Create an index on a table.
///
/// Variants (mutually exclusive):
/// - default — hash-keyed regular index. Equality lookups O(log n).
/// - `unique=true` — hash-keyed unique index with constraint check.
/// - `sorted=true` — value-ordered sorted index. Backs range
///   (`between`/`gt`/`gte`/`lt`/`lte`), `order by field asc + LIMIT
///   K`, and `MIN(field)`. Single-field scalar column only.
///
/// `unique=true` + `sorted=true` is rejected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateIndexOp {
    pub create_index: String,
    pub table: String,
    pub fields: Vec<Vec<String>>,
    #[serde(default)]
    pub unique: bool,
    /// Register as sorted (value-ordered) index for range / order /
    /// min queries. See doc-comment on the struct.
    #[serde(default)]
    pub sorted: bool,
    #[serde(default = "default_repo")]
    pub repo: String,

    /// Index type: "btree" (default), "fts", "functional", "vector".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_type: Option<String>,

    /// FTS tokenizer: "whitespace" (default) or "unicode".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fts_tokenizer: Option<String>,

    /// FTS language hint (for future stemming).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fts_language: Option<String>,

    /// Functional index expression operator: "lower", "upper", "trim",
    /// "length", "substring", "mod", "coalesce", "concat".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub functional_op: Option<String>,

    /// Additional args for functional expr (e.g., mod divisor).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub functional_args: Option<Vec<serde_json::Value>>,

    /// Vector dimension (required for vector indexes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_dim: Option<u32>,

    /// Vector metric: "l2", "cosine" (default), "dot".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_metric: Option<String>,
}

/// Drop an index.
///
/// Requires `hmac` over
/// `b"drop_index\0<db_in_use>\0<repo>\0<table>\0<index>\0<unique:0|1>"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropIndexOp {
    pub drop_index: String,
    pub table: String,
    #[serde(default)]
    pub unique: bool,
    #[serde(default = "default_repo")]
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
}

// ============================================================================
// PER-TABLE BUFFER CONFIG (DDL)
// ============================================================================

/// Full per-table `MemBufferConfig` blob — what the engine writes
/// into `info_store` and what `set_buffer_config` accepts.
///
/// Mirrors `shamir_storage::storage_membuffer::MemBufferConfig`
/// 1:1 — kept separate so the wire DTO doesn't drag the storage
/// crate into clients that just speak JSON. The executor maps
/// this struct into the storage struct on its way in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BufferConfigDto {
    pub max_bytes: usize,
    pub max_entries: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
    pub flush_interval_ms: u64,
    pub flush_batch_size: usize,
}

/// Partial-update payload for `alter_buffer_config`.
///
/// Each field is `Option`-wrapped to mean "leave as is" when
/// absent. `ttl_ms` uses double-option semantics:
///   * key missing → no change,
///   * key present and `null` → clear TTL,
///   * key present and a number → set TTL to that many ms.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BufferConfigPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_entries: Option<usize>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_double_option"
    )]
    pub ttl_ms: Option<Option<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flush_interval_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flush_batch_size: Option<usize>,
}

/// Custom deserializer that distinguishes "absent" from "null".
/// `Some(None)` ↔ explicit null; `Some(Some(v))` ↔ value; `None`
/// is never produced here (serde calls this only when key was
/// present, so the outer `Option::None` is unreachable and means
/// the field was omitted entirely — `serde(default)` handles that).
fn deserialize_double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

/// Persist a full buffer config for a table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetBufferConfigOp {
    pub set_buffer_config: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub config: BufferConfigDto,
}

/// Read the persisted buffer config for a table. Returns `null`
/// in the `config` field when no DDL has set one for this table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetBufferConfigOp {
    pub get_buffer_config: String,
    #[serde(default = "default_repo")]
    pub repo: String,
}

/// Partial-update one or more buffer knobs without re-stating
/// the whole config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlterBufferConfigOp {
    pub alter_buffer_config: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub patch: BufferConfigPatch,
}

// ============================================================================
// TABLE MIGRATION (online engine change)
// ============================================================================

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

/// List databases / repos / tables / indexes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "list")]
pub enum ListOp {
    #[serde(rename = "databases")]
    Databases,
    #[serde(rename = "repos")]
    Repos,
    #[serde(rename = "tables")]
    Tables {
        #[serde(default = "default_repo")]
        repo: String,
    },
    #[serde(rename = "indexes")]
    Indexes {
        table: String,
        #[serde(default = "default_repo")]
        repo: String,
    },
    #[serde(rename = "users")]
    Users,
    #[serde(rename = "roles")]
    Roles,
}
