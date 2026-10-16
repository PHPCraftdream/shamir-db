//! Index-level DDL operations: create / drop index.

use serde::{Deserialize, Serialize};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::admin::types::db_ops::is_false;

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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_fts_tokenizer"
    )]
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_vector_metric"
    )]
    pub vector_metric: Option<String>,

    /// Vector quantization (V5.2 #411): "sq8" for SQ8 scalar quantization
    /// (deferred fit at 256 vectors, then a u8-code HNSW graph with
    /// dequant-rescore). `None` / omitted selects the unquantized f32 path.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_vector_quantization"
    )]
    pub vector_quantization: Option<String>,

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

/// FTS tokenizer DSL values accepted by the engine's tokenizer parser.
fn valid_fts_tokenizer(value: &str) -> bool {
    let ngram_is_valid = value
        .strip_prefix("ngram")
        .is_some_and(|digits| digits.parse::<u8>().is_ok_and(|n| n > 0));
    matches!(value, "whitespace" | "unicode" | "ngram")
        || ngram_is_valid
        || value.strip_prefix("stemmed_").is_some_and(|lang| {
            matches!(
                lang,
                "english"
                    | "en"
                    | "russian"
                    | "ru"
                    | "arabic"
                    | "ar"
                    | "danish"
                    | "da"
                    | "dutch"
                    | "nl"
                    | "finnish"
                    | "fi"
                    | "french"
                    | "fr"
                    | "german"
                    | "de"
                    | "greek"
                    | "el"
                    | "hungarian"
                    | "hu"
                    | "italian"
                    | "it"
                    | "norwegian"
                    | "no"
                    | "portuguese"
                    | "pt"
                    | "romanian"
                    | "ro"
                    | "spanish"
                    | "es"
                    | "swedish"
                    | "sv"
                    | "tamil"
                    | "ta"
                    | "turkish"
                    | "tr"
            )
        })
}

fn deserialize_fts_tokenizer<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)
        .map_err(|error| serde::de::Error::custom(format!("fts_tokenizer: {error}")))?;
    if let Some(tokenizer) = value.as_deref() {
        if !valid_fts_tokenizer(tokenizer) {
            return Err(serde::de::Error::custom(format!(
                "fts_tokenizer: unknown value '{tokenizer}'"
            )));
        }
    }
    Ok(value)
}

fn deserialize_vector_metric<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)
        .map_err(|error| serde::de::Error::custom(format!("vector_metric: {error}")))?;
    if let Some(metric) = value.as_deref() {
        if !matches!(metric, "l2" | "dot" | "cosine") {
            return Err(serde::de::Error::custom(format!(
                "vector_metric: unknown value '{}' (expected l2, dot, or cosine)",
                metric
            )));
        }
    }
    Ok(value)
}

fn deserialize_vector_quantization<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)
        .map_err(|error| serde::de::Error::custom(format!("vector_quantization: {error}")))?;
    if let Some(quantization) = value.as_deref() {
        if !quantization.eq_ignore_ascii_case("sq8") {
            return Err(serde::de::Error::custom(format!(
                "vector_quantization: unknown value '{}' (expected sq8)",
                quantization
            )));
        }
    }
    Ok(value)
}

/// Rename an index on a table (in-place rekey, no data loss).
///
/// Wire shape: `{ "rename_index": "old_name", "to": "new_name",
///               "table": "users", "repo": "main" }`.
///
/// Semantics:
/// - The index posting-list entries are migrated from the old interned-name
///   prefix to the new one. All index types are handled: regular (hash),
///   unique, sorted, and index2 (fts / functional / vector). index2 backends
///   are rebuilt under the new name because their data is opaque to a raw
///   key-rewrite.
/// - Refuses when the destination name is already taken by any index on the
///   same table.
/// - Refuses when the source index does not exist (unless `if_exists = true`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameIndexOp {
    /// Old index name.
    pub rename_index: String,
    /// New index name.
    pub to: String,
    /// Parent table.
    pub table: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    /// When `true`, renaming a non-existent source index (or one whose
    /// parent db/table is missing) is a silent no-op returning
    /// `{"renamed_index": <source>, "existed": false}` instead of an
    /// error. The destination-occupied rejection is NOT affected by this
    /// flag — it stays a hard error regardless.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_exists: bool,
    /// Client-supplied correlation ID. If present, the server uses this as
    /// the `op_id` for the operation, enabling the client to poll by this ID
    /// even if the response is lost (crash or disconnect). If absent, the
    /// server mints a fresh `RecordId::new()` (backward compatible).
    ///
    /// Idempotent retry: if this field is set and a status record already
    /// exists for this `request_id`, the server short-circuits and returns
    /// the existing status instead of re-executing the mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RecordId>,
}

/// Drop an index.
///
/// Requires `hmac` over
/// `b"drop_index\0<db_in_use>\0<repo>\0<table>\0<index>"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropIndexOp {
    pub drop_index: String,
    pub table: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
    /// When `true`, a missing db, table, or index is a silent no-op
    /// returning `{"existed": false}` instead of an error. When `false`
    /// (the default), any missing entity is a hard `Err`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_exists: bool,
    /// Client-supplied correlation ID. If present, the server uses this as
    /// the `op_id` for the operation, enabling the client to poll by this ID
    /// even if the response is lost (crash or disconnect). If absent, the
    /// server mints a fresh `RecordId::new()` (backward compatible).
    ///
    /// Idempotent retry: if this field is set and a status record already
    /// exists for this `request_id`, the server short-circuits and returns
    /// the existing status instead of re-executing the mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RecordId>,
}
