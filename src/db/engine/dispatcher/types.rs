use crate::types::common::TMap;
use serde::{Serialize, Deserialize};

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
    pub paths: Vec<Vec<String>>,
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

    /// Создаёт из строки
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Canopy" => Some(Self::Canopy),
            "Fjall" => Some(Self::Fjall),
            "Cached" => Some(Self::Cached),
            "Memory" => Some(Self::Memory),
            "Nebari" => Some(Self::Nebari),
            "Persy" => Some(Self::Persy),
            "Redb" => Some(Self::Redb),
            "Sled" => Some(Self::Sled),
            _ => None,
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
        Self::from_str(s).ok_or_else(|| format!("Invalid StorageType: {}", s))
    }
}
