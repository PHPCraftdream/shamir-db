#[cfg(test)]

use crate::core::db_config::{DbConfig, DbRepoConfig, DbTableConfig, StorageType};
use crate::core::db_config_loader::DbConfigLoader;
use crate::types::common::new_map;

#[test]
fn test_config_validation_empty_data_dir() {
    let mut repos = new_map();
    let mut tables = new_map();
    tables.insert(
        "users".to_string(),
        DbTableConfig {
            indexes: new_map(),
            indexes_unique: new_map(),
        },
    );

    repos.insert(
        "default".to_string(),
        DbRepoConfig {
            tables,
            storage_type: StorageType::Redb,
            ram_cached: true,
        },
    );

    let config = DbConfig {
        data_dir: String::new(),
        repos,
    };

    let result = DbConfigLoader::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("data_dir"));
}

#[test]
fn test_config_validation_empty_repos() {
    let config = DbConfig {
        data_dir: "./data".to_string(),
        repos: new_map(),
    };

    let result = DbConfigLoader::validate_config(&config);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("at least one repository"));
}
