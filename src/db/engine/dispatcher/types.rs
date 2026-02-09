use crate::types::common::TMap;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbConfig {
    pub data_dir: String,
    pub wal_enabled: bool,
    pub repos: TMap<String, RepoConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub path: String,
    pub tables: TMap<String, TableConfig>,
    pub storage_type: StorageType,
    pub ram_cached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageType {
    Canopy,
    Fjall,
    Cached,
    Memory,
    Nebari,
    Persy,
    Redb,
    Sled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableConfig {
    pub indexes: TMap<String, IndexConfig>,
    pub indexes_unique: TMap<String, IndexConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexConfig {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnConfig {
    pub name: String,
    pub data_type: DataType,
    pub primary_key: bool,
    pub indexed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DataType {
    String,
    Bigint,
    Integer,
    Float,
    Boolean,
    Decimal,
    DateTime,
    Binary,
}
