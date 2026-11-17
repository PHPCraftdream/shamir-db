use serde::{Deserialize, Serialize};

use super::values::LogicalValue;

/// Portable database row projection. Only the explicitly listed logical
/// settings are in scope; credentials, users/roles tables and replication
/// metadata do not have representable variants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseItem {
    pub name: String,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub mode: u16,
    pub settings: Vec<SettingItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingItem {
    pub key: SettingKey,
    pub value: LogicalValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingKey {
    RootMeta,
    FunctionNamespaceMeta,
}

/// Portable repository configuration; physical locations and lineage are
/// intentionally absent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryItem {
    pub name: String,
    pub engine: RepositoryEngine,
    pub durability: Durability,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub mode: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepositoryEngine {
    InMemory,
    Fjall,
    Hybrid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Durability {
    Default,
    Durable,
    Relaxed,
}

/// Table configuration includes its buffer TTL verbatim and no buffer cache.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableItem {
    pub repository: String,
    pub name: String,
    pub enable_indexes: bool,
    pub schema_version: u64,
    pub schema: LogicalValue,
    pub schema_validator: Option<String>,
    pub buffer_ttl_ms: Option<u64>,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub mode: u16,
}
