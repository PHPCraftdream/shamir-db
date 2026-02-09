use crate::types::common::TMap;

pub struct DbConfig {
    pub repos: TMap<String, RepoConfig>,
}

pub struct RepoConfig {
    pub tables: TMap<String, TableConfig>,
    pub storage_type: StorageType,
    pub ram_cached: bool,
}

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

pub struct TableConfig {
    pub indexes: TMap<String, IndexConfig>,
    pub indexes_unique: TMap<String, IndexConfig>,
}

pub struct IndexConfig {
    pub paths: Vec<String>,
}