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
        if config.data_dir.is_empty() {
            anyhow::bail!("Config must specify a data_dir");
        }

        if config.repos.is_empty() {
            anyhow::bail!("Config must contain at least one repository");
        }

        for (repo_name, repo_config) in &config.repos {
            if repo_config.tables.is_empty() {
                anyhow::bail!("Repository '{}' must contain at least one table", repo_name);
            }

            Self::validate_repo_config(repo_name, repo_config)?;
        }

        Ok(())
    }

    fn validate_repo_config(repo_name: &str, repo_config: &RepoConfig) -> Result<()> {
        for (table_name, table_config) in &repo_config.tables {
            if table_config.indexes.is_empty() && table_config.indexes_unique.is_empty() {
                anyhow::bail!(
                    "Table '{}.{}' must have at least one index",
                    repo_name,
                    table_name
                );
            }

            for (index_name, index_config) in &table_config.indexes {
                if index_config.paths.is_empty() {
                    anyhow::bail!(
                        "Index '{}.{}.{}' must have at least one path",
                        repo_name,
                        table_name,
                        index_name
                    );
                }
            }

            for (index_name, index_config) in &table_config.indexes_unique {
                if index_config.paths.is_empty() {
                    anyhow::bail!(
                        "Unique index '{}.{}.{}' must have at least one path",
                        repo_name,
                        table_name,
                        index_name
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::db::engine::dispatcher::StorageType;
    use super::*;
    use crate::types::common::new_map;
    use crate::db::engine::dispatcher::types::{IndexConfig, TableConfig};

    #[test]
    fn test_config_roundtrip_yaml() {
        let mut repos = new_map();
        let mut tables = new_map();
        let mut indexes = new_map();
        indexes.insert("email_idx".to_string(), IndexConfig {
            paths: vec!["email".to_string()],
        });

        let tables_config = TableConfig {
            indexes,
            indexes_unique: new_map(),
        };

        tables.insert("users".to_string(), tables_config);

        repos.insert("default".to_string(), RepoConfig {
            tables,
            storage_type: StorageType::Redb,
            ram_cached: true,
        });

        let config = DbConfig {
            data_dir: "./data".to_string(),
            repos,
        };

        let yaml = serde_yaml::to_string(&config).unwrap();

        let deserialized: DbConfig = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(config.repos.len(), deserialized.repos.len());

        let repo = deserialized.repos.get("default").unwrap();
        assert_eq!(repo.tables.len(), 1);
        assert!(repo.ram_cached);
        assert!(matches!(repo.storage_type, StorageType::Redb));

        let table = repo.tables.get("users").unwrap();
        assert_eq!(table.indexes.len(), 1);
        assert_eq!(table.indexes["email_idx"].paths, vec!["email"]);
    }

    #[test]
    fn test_config_validation_empty_repos() {
        let config = DbConfig {
            data_dir: "./data".to_string(),
            repos: new_map(),
        };

        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least one repository"));
    }

    #[test]
    fn test_config_validation_no_tables() {
        let mut repos = new_map();
        repos.insert("default".to_string(), RepoConfig {
            tables: new_map(),
            storage_type: StorageType::Redb,
            ram_cached: true,
        });

        let config = DbConfig {
            data_dir: "./data".to_string(),
            repos,
        };

        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least one table"));
    }

    #[test]
    fn test_config_validation_no_indexes() {
        let mut repos = new_map();
        let mut tables = new_map();
        tables.insert("users".to_string(), TableConfig {
            indexes: new_map(),
            indexes_unique: new_map(),
        });

        repos.insert("default".to_string(), RepoConfig {
            tables,
            storage_type: StorageType::Redb,
            ram_cached: true,
        });

        let config = DbConfig {
            data_dir: "./data".to_string(),
            repos,
        };

        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least one index"));
    }

    #[test]
    fn test_config_validation_empty_data_dir() {
        let mut repos = new_map();
        let mut tables = new_map();
        tables.insert("users".to_string(), TableConfig {
            indexes: new_map(),
            indexes_unique: new_map(),
        });

        repos.insert("default".to_string(), RepoConfig {
            tables,
            storage_type: StorageType::Redb,
            ram_cached: true,
        });

        let config = DbConfig {
            data_dir: String::new(),
            repos,
        };

        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("data_dir"));
    }
}
