use crate::db::engine::dispatcher::types::{DbRepoConfig, DbTableConfig, IndexConfig, StorageType};
use crate::db::engine::dispatcher::DbConfig;
use crate::types::common::new_map;

#[test]
fn test_config_roundtrip_yaml() {
    let mut repos = new_map();
    let mut tables = new_map();
    let mut indexes = new_map();
    indexes.insert(
        "email_idx".to_string(),
        IndexConfig {
            paths: vec![vec!["email".to_string()]],
        },
    );

    let tables_config = DbTableConfig {
        indexes,
        indexes_unique: new_map(),
    };

    tables.insert("users".to_string(), tables_config);

    repos.insert(
        "default".to_string(),
        DbRepoConfig {
            tables,
            storage_type: StorageType::Redb,
            ram_cached: true,
        },
    );

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
    assert_eq!(
        table.indexes["email_idx"].paths,
        vec![vec!["email".to_string()]]
    );
}
