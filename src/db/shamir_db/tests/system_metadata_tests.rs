use crate::db::engine::repo::{BoxRepo, RepoConfig};
use crate::db::engine::table::TableConfig;
use crate::db::shamir_db::ShamirDb;
use crate::db::storage::storage_in_memory::InMemoryRepo;
use std::sync::Arc;

// ============================================================================
// System repo tests - metadata persistence
// ============================================================================

#[tokio::test]
async fn test_system_repo_exists_after_creation() {
    let shamir = ShamirDb::new();

    assert!(shamir.has_db("__system__"));
}

#[tokio::test]
async fn test_create_db_persists_to_system() {
    let shamir = ShamirDb::new();

    shamir.create_db("production").await;

    let db_names = shamir.list_databases_metadata();
    assert!(db_names.contains(&"production".to_string()));
    assert!(!db_names.contains(&"__system__".to_string()));
}

#[tokio::test]
async fn test_remove_db_removes_from_system() {
    let shamir = ShamirDb::new();

    shamir.create_db("production").await;
    assert!(shamir
        .list_databases_metadata()
        .contains(&"production".to_string()));

    shamir.remove_db("production").await;

    let db_names = shamir.list_databases_metadata();
    assert!(!db_names.contains(&"production".to_string()));
}

#[tokio::test]
async fn test_add_repo_persists_to_system() {
    let shamir = ShamirDb::new();
    shamir.create_db("production").await;

    let repo = Arc::new(InMemoryRepo::new());
    let config =
        RepoConfig::new("users_db", BoxRepo::InMemory(repo)).add_table(TableConfig::new("users"));

    shamir.add_repo("production", config).await.unwrap();

    let repo_records = shamir.list_repositories_metadata("production");
    assert_eq!(repo_records.len(), 1);
    assert_eq!(repo_records[0].repo_name, "users_db");
    assert_eq!(repo_records[0].db_name, "production");
}

#[tokio::test]
async fn test_list_tables_for_admin() {
    let shamir = ShamirDb::new();
    shamir.create_db("production").await;

    let repo = Arc::new(InMemoryRepo::new());
    let config = RepoConfig::new("users_db", BoxRepo::InMemory(repo))
        .add_table(TableConfig::new("users"))
        .add_table(TableConfig::new("sessions"))
        .add_table(TableConfig::new("tokens"));

    shamir.add_repo("production", config).await.unwrap();

    let db = shamir.get_db("production").unwrap();
    let tables = db.list_tables("users_db").unwrap();
    assert_eq!(tables.len(), 3);
    assert!(tables.contains(&"users".to_string()));
    assert!(tables.contains(&"sessions".to_string()));
    assert!(tables.contains(&"tokens".to_string()));
}

#[tokio::test]
async fn test_restore_from_system_metadata() {
    let shamir1 = ShamirDb::new();

    shamir1.create_db("production").await;

    let repo = Arc::new(InMemoryRepo::new());
    let config =
        RepoConfig::new("users_db", BoxRepo::InMemory(repo)).add_table(TableConfig::new("users"));

    shamir1.add_repo("production", config).await.unwrap();

    let cloned = shamir1.clone();
    assert_eq!(cloned.db_count(), 2);

    let restored_db = cloned.get_db("production").unwrap();
    assert!(restored_db.has_repo("users_db"));
}

#[tokio::test]
async fn test_system_metadata_repo_has_tables() {
    let shamir = ShamirDb::new();

    let system_db = shamir.get_db("__system__").unwrap();
    let repos = system_db.list_repos();

    assert!(repos.contains(&"metadata".to_string()));

    let tables = system_db.list_tables("metadata").unwrap();
    assert!(tables.contains(&"databases".to_string()));
    assert!(tables.contains(&"repositories".to_string()));
}

#[tokio::test]
async fn test_multiple_repos_in_same_db() {
    let shamir = ShamirDb::new();
    shamir.create_db("production").await;

    let repo1 = Arc::new(InMemoryRepo::new());
    let config1 =
        RepoConfig::new("users_db", BoxRepo::InMemory(repo1)).add_table(TableConfig::new("users"));

    let repo2 = Arc::new(InMemoryRepo::new());
    let config2 = RepoConfig::new("products_db", BoxRepo::InMemory(repo2))
        .add_table(TableConfig::new("products"));

    shamir.add_repo("production", config1).await.unwrap();
    shamir.add_repo("production", config2).await.unwrap();

    let repo_records = shamir.list_repositories_metadata("production");
    assert_eq!(repo_records.len(), 2);

    let repo_names: Vec<&str> = repo_records.iter().map(|r| r.repo_name.as_str()).collect();
    assert!(repo_names.contains(&"users_db"));
    assert!(repo_names.contains(&"products_db"));
}
