use crate::db::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::db::engine::table::TableConfig;
use crate::db::shamir_db::ShamirDb;

// ============================================================================
// System store persistence tests
// ============================================================================

#[tokio::test]
async fn test_create_db_persists() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    shamir.create_db("production").await;
    assert!(shamir.has_db("production"));

    // Verify persisted in system store
    let dbs = shamir.system_store().load_databases().await.unwrap();
    assert!(dbs.iter().any(|d| d["name"] == "production"));
}

#[tokio::test]
async fn test_remove_db_removes_from_system_store() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    shamir.create_db("temp").await;
    shamir.remove_db("temp").await;

    let dbs = shamir.system_store().load_databases().await.unwrap();
    assert!(!dbs.iter().any(|d| d["name"] == "temp"));
}

#[tokio::test]
async fn test_add_repo_persists() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("production").await;

    let config = RepoConfig::new("users_db", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    shamir.add_repo("production", config).await.unwrap();

    let repos = shamir.system_store().load_repositories().await.unwrap();
    assert!(repos.iter().any(|r| r["repo_name"] == "users_db" && r["db_name"] == "production"));
}

#[tokio::test]
async fn test_remove_repo_removes_from_system_store() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("production").await;

    let config = RepoConfig::new("temp_repo", BoxRepoFactory::in_memory());
    shamir.add_repo("production", config).await.unwrap();
    shamir.remove_repo("production", "temp_repo").await;

    let repos = shamir.system_store().load_repositories().await.unwrap();
    assert!(!repos.iter().any(|r| r["repo_name"] == "temp_repo"));
}

#[tokio::test]
async fn test_system_store_has_tables() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // System store should have settings, users, roles tables accessible
    let settings = shamir.system_store().load_setting("nonexistent").await.unwrap();
    assert!(settings.is_none());
}

#[tokio::test]
async fn test_settings_persistence() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    shamir.system_store().save_setting("max_connections", &serde_json::json!(100)).await.unwrap();

    let val = shamir.system_store().load_setting("max_connections").await.unwrap();
    assert_eq!(val, Some(serde_json::json!(100)));
}

#[tokio::test]
async fn test_multiple_repos_persist() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("production").await;

    let config1 = RepoConfig::new("users_db", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    let config2 = RepoConfig::new("products_db", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("products"));

    shamir.add_repo("production", config1).await.unwrap();
    shamir.add_repo("production", config2).await.unwrap();

    let repos = shamir.system_store().load_repositories().await.unwrap();
    let prod_repos: Vec<_> = repos.iter().filter(|r| r["db_name"] == "production").collect();
    assert_eq!(prod_repos.len(), 2);
}
