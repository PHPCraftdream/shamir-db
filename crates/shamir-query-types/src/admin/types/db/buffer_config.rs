//! Per-table memory-buffer configuration DTOs and DDL operations.

use serde::{Deserialize, Serialize};

fn default_repo() -> String {
    "main".to_string()
}

/// Full per-table `MemBufferConfig` blob — what the engine writes
/// into `info_store` and what `set_buffer_config` accepts.
///
/// Mirrors `shamir_storage::storage_membuffer::MemBufferConfig`
/// 1:1 — kept separate so the wire DTO doesn't drag the storage
/// crate into clients. The executor maps
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
