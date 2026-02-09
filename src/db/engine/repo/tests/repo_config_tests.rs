use crate::db::engine::repo::repo_config::RepoConfig;
use crate::db::engine::repo::repo_types::BoxRepo;
use crate::db::engine::table::TableConfig;
use crate::db::storage::storage_in_memory::InMemoryRepo;
use std::sync::Arc;

#[test]
fn test_repo_config_new() {
    let repo = BoxRepo::InMemory(Arc::new(InMemoryRepo::new()));
    let config = RepoConfig::new("test", repo);
    assert_eq!(config.name, "test");
    assert!(config.tables.is_empty());
}

#[test]
fn test_repo_config_add_table() {
    let repo = BoxRepo::InMemory(Arc::new(InMemoryRepo::new()));
    let config = RepoConfig::new("test", repo)
        .add_table(TableConfig::new("users"))
        .add_table(TableConfig::new("products"));

    assert_eq!(config.tables.len(), 2);
    assert_eq!(config.tables[0].name, "users");
    assert_eq!(config.tables[1].name, "products");
}

#[test]
fn test_repo_config_add_tables() {
    let repo = BoxRepo::InMemory(Arc::new(InMemoryRepo::new()));
    let tables = vec![
        TableConfig::new("users"),
        TableConfig::new("products"),
        TableConfig::new("orders"),
    ];
    let config = RepoConfig::new("test", repo).add_tables(tables);

    assert_eq!(config.tables.len(), 3);
}

#[test]
fn test_repo_config_clone() {
    let repo = BoxRepo::InMemory(Arc::new(InMemoryRepo::new()));
    let config1 = RepoConfig::new("test", repo.clone()).add_table(TableConfig::new("users"));
    let config2 = config1.clone();

    assert_eq!(config1.name, config2.name);
    assert_eq!(config1.tables.len(), config2.tables.len());
}
