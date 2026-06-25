//! Index-level DDL operations: create / drop index.

use serde::{Deserialize, Serialize};
use shamir_types::types::value::QueryValue;

use super::db_ops::is_false;

fn default_repo() -> String {
    "main".to_string()
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
    pub functional_args: Option<Vec<QueryValue>>,

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
    /// When `true`, dropping a non-existent index (or one whose parent
    /// db/table is missing) is a silent no-op returning
    /// `{"existed": false}` instead of an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_exists: bool,
}
