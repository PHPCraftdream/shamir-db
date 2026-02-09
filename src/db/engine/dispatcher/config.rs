use crate::db::engine::dispatcher::types::{DbConfig, RepoConfig};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub struct ConfigLoader;

impl ConfigLoader {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<DbConfig> {
        let content = fs::read_to_string(path.as_ref())
            .context("Failed to read config file")?;

        let config: DbConfig = serde_yaml::from_str(&content)
            .context("Failed to parse YAML config")?;

        Self::validate_config(&config)?;
        Ok(config)
    }

    pub fn save_to_file<P: AsRef<Path>>(path: P, config: &DbConfig) -> Result<()> {
        let yaml = serde_yaml::to_string(config)
            .context("Failed to serialize config to YAML")?;

        let temp_path = path.as_ref().with_extension("yaml.tmp");
        fs::write(&temp_path, yaml)
            .context("Failed to write temp config file")?;

        fs::rename(&temp_path, path.as_ref())
            .context("Failed to rename temp config file")?;

        Ok(())
    }

    fn validate_config(config: &DbConfig) -> Result<()> {
        if config.repos.is_empty() {
            anyhow::bail!("Config must contain at least one repository");
        }

        for (repo_name, repo_config) in &config.repos {
            if repo_config.tables.is_empty() {
                anyhow::bail!("Repository '{}' must contain at least one table", repo_name);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::engine::dispatcher::types::{IndexConfig, StorageType, TableConfig};
    use crate::types::common::new_map;

    #[test]
    fn test_config_roundtrip_yaml() {
        let mut repos = new_map();
        let mut tables = new_map();
        let mut indexes = new_map();
        indexes.insert("email_idx".to_string(), IndexConfig {
            paths: vec!["email".to_string()],
        });

        tables.insert("users".to_string(), TableConfig {
            indexes,
            indexes_unique: new_map(),
        });

        repos.insert("default".to_string(), RepoConfig {
            path: "./data/default".to_string(),
            tables,
            storage_type: StorageType::Redb,
            ram_cached: true,
        });

        let config = DbConfig {
            data_dir: "./data".to_string(),
            wal_enabled: true,
            repos,
        };

        // Serialize
        let yaml = serde_yaml::to_string(&config).unwrap();

        // Deserialize
        let deserialized: DbConfig = serde_yaml::from_str(&yaml).unwrap();

        // Verify
        assert_eq!(config.data_dir, deserialized.data_dir);
        assert_eq!(config.wal_enabled, deserialized.wal_enabled);
        assert_eq!(config.repos.len(), deserialized.repos.len());

        let repo = deserialized.repos.get("default").unwrap();
        assert_eq!(repo.path, "./data/default");
        assert_eq!(repo.tables.len(), 1);
    }

    #[test]
    fn test_config_validation_empty_repos() {
        let config = DbConfig {
            data_dir: "./data".to_string(),
            wal_enabled: true,
            repos: new_map(),
        };

        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least one repository"));
    }

    #[test]
    fn test_config_validation_no_primary_key() {
        let mut repos = new_map();
        let mut tables = new_map();

        tables.insert("users".to_string(), TableConfig {
            indexes: new_map(),
            indexes_unique: new_map(),
        });

        repos.insert("default".to_string(), RepoConfig {
            path: "./data/default".to_string(),
            tables,
            storage_type: StorageType::Redb,
            ram_cached: true,
        });

        let config = DbConfig {
            data_dir: "./data".to_string(),
            wal_enabled: true,
            repos,
        };

        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("primary key"));
    }
}
