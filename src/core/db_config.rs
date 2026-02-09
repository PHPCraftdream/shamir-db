use crate::types::common::TMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbConfig {
    pub data_dir: String,
    pub repos: TMap<String, DbRepoConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbRepoConfig {
    pub tables: TMap<String, DbTableConfig>,
    pub storage_type: StorageType,
    pub ram_cached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbTableConfig {
    pub indexes: TMap<String, IndexConfig>,
    pub indexes_unique: TMap<String, IndexConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexConfig {
    pub paths: Vec<String>,
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

impl StorageType {
    /// Возвращает строковое представление
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Canopy => "Canopy",
            Self::Fjall => "Fjall",
            Self::Cached => "Cached",
            Self::Memory => "Memory",
            Self::Nebari => "Nebari",
            Self::Persy => "Persy",
            Self::Redb => "Redb",
            Self::Sled => "Sled",
        }
    }
}

// Для использования в Display
impl std::fmt::Display for StorageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// Для парсинга из строки (FromStr trait)
impl std::str::FromStr for StorageType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Canopy" => Ok(Self::Canopy),
            "Fjall" => Ok(Self::Fjall),
            "Cached" => Ok(Self::Cached),
            "Memory" => Ok(Self::Memory),
            "Nebari" => Ok(Self::Nebari),
            "Persy" => Ok(Self::Persy),
            "Redb" => Ok(Self::Redb),
            "Sled" => Ok(Self::Sled),
            _ => Err(format!("Invalid StorageType: {}", s)),
        }
    }
}
