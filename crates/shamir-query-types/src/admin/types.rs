//! Admin (DDL) operation types.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// Serde skip-serializing-if helper: omit `false` booleans from the wire.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Create a new database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateDbOp {
    pub create_db: String,
    /// When `true`, a pre-existing database with the same name is NOT an
    /// error — the operation returns `{"created": false, "existed": true}`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_not_exists: bool,
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
    /// When `true`, all repositories (and their tables) inside the database
    /// are removed recursively before the database itself is dropped.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cascade: bool,
}

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

/// Create a table in a repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTableOp {
    pub create_table: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    /// When `true`, a pre-existing table with the same name is NOT an
    /// error — the operation returns `{"created": false, "existed": true}`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_not_exists: bool,
    /// Optional per-table history retention applied at creation time.
    /// `None` (absent on the wire) = CurrentOnly — today's default
    /// behaviour (no history retained). See [`Retention`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<Retention>,
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

    /// Covering index: extra fields whose values are stored directly in the
    /// index entry so a covered range query is served from the index alone
    /// (no data-store fetch). Only meaningful for `sorted` indexes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<Vec<String>>,

    /// When `true`, a pre-existing index with the same name is NOT an
    /// error — the operation returns `{"created": false, "existed": true}`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_not_exists: bool,
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

// ============================================================================
// FUNCTION DDL
// ============================================================================

/// Create (or replace) a stored function from Rust source or pre-compiled WASM.
///
/// Exactly one of `source` or `wasm` must be provided. `wasm` is the raw
/// binary bytes (base64-encoded on the wire).
///
/// ```json
/// { "create_function": "my_fn", "source": "pub fn shamir_call …", "replace": false }
/// { "create_function": "my_fn", "wasm": "<base64>", "replace": true }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateFunctionOp {
    pub create_function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm: Option<String>,
    #[serde(default)]
    pub replace: bool,
}

/// Drop a stored function by name.
///
/// ```json
/// { "drop_function": "my_fn" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropFunctionOp {
    pub drop_function: String,
}

/// Rename a stored function.
///
/// ```json
/// { "rename_function": "old_name", "to": "new_name" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameFunctionOp {
    pub rename_function: String,
    pub to: String,
}

// ============================================================================
// VALIDATOR DDL
// ============================================================================

/// Create (or replace) a validator from Rust source or pre-compiled WASM.
///
/// ```json
/// { "create_validator": "v_age", "source": "pub fn shamir_call …", "replace": false }
/// { "create_validator": "v_age", "wasm": "<base64>", "replace": true }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateValidatorOp {
    pub create_validator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm: Option<String>,
    #[serde(default)]
    pub replace: bool,
}

/// Drop a validator by name.
///
/// ```json
/// { "drop_validator": "v_age" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropValidatorOp {
    pub drop_validator: String,
}

/// Rename a validator.
///
/// ```json
/// { "rename_validator": "old_name", "to": "new_name" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameValidatorOp {
    pub rename_validator: String,
    pub to: String,
}

/// Bind a validator to a table on specified write operations.
///
/// ```json
/// {
///   "bind_validator": "v_age",
///   "table": { "db": "testdb", "repo": "main", "table": "users" },
///   "ops": ["insert", "update"],
///   "priority": 1500
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindValidatorOp {
    pub bind_validator: String,
    pub db: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub table: String,
    pub ops: Vec<crate::WriteOp>,
    pub priority: u16,
}

/// Unbind a validator from a table.
///
/// ```json
/// { "unbind_validator": "v_age", "table": { "db": "testdb", "repo": "main", "table": "users" } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnbindValidatorOp {
    pub unbind_validator: String,
    pub db: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub table: String,
}

/// List validator bindings for a table.
///
/// ```json
/// { "list_validators": "users", "db": "testdb", "repo": "main" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListValidatorsOp {
    pub list_validators: String,
    pub db: String,
    #[serde(default = "default_repo")]
    pub repo: String,
}

// ============================================================================
// FUNCTION FOLDER DDL
// ============================================================================

/// Create a function folder by path segments.
///
/// ```json
/// { "create_function_folder": ["reports", "daily"] }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateFunctionFolderOp {
    pub create_function_folder: Vec<String>,
}

// ============================================================================
// TEMPORAL — retention + imperative history purge (T2 DTOs)
// ============================================================================

/// Per-table history retention. Three ORTHOGONAL optional knobs, each set
/// independently. All-`None` = Forever; `max_count: Some(0)` = CurrentOnly.
/// Caps intersect (tighter prunes); the floor (`min_count`) overrides
/// `max_age`. See `TEMPORAL.md` §3.
///
/// `Default` = all-`None` = Forever; the engine treats "table created
/// without retention" as CurrentOnly — that default lives at the table
/// layer in T3, not here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Retention {
    /// CAP by age (seconds). `None` = no age cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_secs: Option<u64>,
    /// CAP by version count/key. `None` = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_count: Option<u64>,
    /// FLOOR: always keep >= this many recent versions, past `max_age`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_count: Option<u64>,
}

impl Retention {
    /// CurrentOnly — no history is retained.
    pub fn current_only() -> Self {
        Self {
            max_count: Some(0),
            ..Default::default()
        }
    }

    /// Validate that `min_count <= max_count` when both are set.
    pub fn validate(&self) -> Result<(), String> {
        match (self.min_count, self.max_count) {
            (Some(min), Some(max)) if min > max => Err(format!(
                "retention min_count ({min}) must be <= max_count ({max})"
            )),
            _ => Ok(()),
        }
    }

    /// True when this retention means "keep no history at all" —
    /// `max_count == Some(0)` with no age floor or floor override.
    pub fn is_current_only(&self) -> bool {
        matches!(self.max_count, Some(0)) && self.max_age_secs.is_none() && self.min_count.is_none()
    }
}

/// Imperative history purge scope — the manual twin of vacuum.
/// Both predicates are epoch-millis / age-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PurgeScope {
    /// Purge history older than this timestamp (epoch-millis).
    OlderThan { timestamp: u64 },
    /// Purge history older than this age (seconds).
    OlderThanAge { age_secs: u64 },
}

/// Imperative history purge for a table.
///
/// ```json
/// { "purge_history": "users", "repo": "main", "scope": { "older_than_age": { "age_secs": 86400 } } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PurgeHistoryOp {
    pub purge_history: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub scope: PurgeScope,
}

/// Change a live table's history-retention policy on the fly (T3).
///
/// The discriminator key `set_retention` holds the table name. The
/// policy is applied via a lock-free `ArcSwap` swap — no data migration,
/// no reshape; subsequent writes are governed by the new policy.
///
/// ```json
/// { "set_retention": "users", "repo": "main", "retention": { "max_count": 5 } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetRetentionOp {
    pub set_retention: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub retention: Retention,
}

/// List databases / repos / tables / indexes / functions / validators / function_folders.
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
    /// List all registered functions. Optionally filter by folder prefix.
    #[serde(rename = "functions")]
    Functions {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<String>,
    },
    /// List all registered validators (id + name + bound tables).
    #[serde(rename = "validators")]
    Validators,
    /// List explicitly created function folders. Optionally filter by parent.
    #[serde(rename = "function_folders")]
    FunctionFolders {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
}
