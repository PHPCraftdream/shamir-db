use crate::db::engine::repo::repo_config::RepoConfig;
use crate::db::engine::repo::repo_manager::RepoManager;
use crate::db::engine::repo::repo_types::BoxRepo;
use crate::db::storage::storage_in_memory::InMemoryRepo;
use std::sync::Arc;

#[tokio::test]
async fn test_repo_manager_new() {
    let manager = RepoManager::new();
    assert_eq!(manager.repo_count(), 0);
    assert!(manager.list_repos().is_empty());
}

#[tokio::test]
async fn test_add_repo() {
    let mut manager = RepoManager::new();
    let repo = Arc::new(InMemoryRepo::new());
    let config = RepoConfig::new("test", BoxRepo::InMemory(repo));

    let old = manager.add_repo(config);
    assert!(old.is_none());
    assert_eq!(manager.repo_count(), 1);
    assert!(manager.has_repo("test"));
}

#[tokio::test]
async fn test_get_repo() {
    let mut manager = RepoManager::new();
    let repo = Arc::new(InMemoryRepo::new());
    let config = RepoConfig::new("test", BoxRepo::InMemory(repo));
    manager.add_repo(config);

    let retrieved = manager.get_repo_config("test").unwrap();
    assert_eq!(retrieved.name, "test");
    assert_eq!(manager.repo_count(), 1);
}

#[tokio::test]
async fn test_get_repo_not_found() {
    let manager = RepoManager::new();
    let result = manager.get_repo_config("nonexistent");
    assert!(result.is_err());
    match result {
        Err(crate::db::error::DbError::NotFound(msg)) => {
            assert!(msg.contains("nonexistent"));
            assert!(msg.contains("not found"));
        }
        _ => panic!("Expected NotFound error"),
    }
}

#[tokio::test]
async fn test_remove_repo() {
    let mut manager = RepoManager::new();
    let repo = Arc::new(InMemoryRepo::new());
    let config = RepoConfig::new("test", BoxRepo::InMemory(repo));
    manager.add_repo(config);

    let removed = manager.remove_repo("test").unwrap();
    assert_eq!(removed.name, "test");
    assert_eq!(manager.repo_count(), 0);
    assert!(!manager.has_repo("test"));
}

#[tokio::test]
async fn test_remove_repo_not_found() {
    let mut manager = RepoManager::new();
    let result = manager.remove_repo("nonexistent");
    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_repos() {
    let mut manager = RepoManager::new();

    manager.add_repo(RepoConfig::new("repo1", BoxRepo::InMemory(Arc::new(InMemoryRepo::new()))));
    manager.add_repo(RepoConfig::new("repo2", BoxRepo::InMemory(Arc::new(InMemoryRepo::new()))));
    manager.add_repo(RepoConfig::new("repo3", BoxRepo::InMemory(Arc::new(InMemoryRepo::new()))));

    let names = manager.list_repos();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"repo1".to_string()));
    assert!(names.contains(&"repo2".to_string()));
    assert!(names.contains(&"repo3".to_string()));
}

#[tokio::test]
async fn test_replace_repo() {
    let mut manager = RepoManager::new();
    let repo1 = Arc::new(InMemoryRepo::new());
    let repo2 = Arc::new(InMemoryRepo::new());

    manager.add_repo(RepoConfig::new("test", BoxRepo::InMemory(repo1)));
    let old = manager.add_repo(RepoConfig::new("test", BoxRepo::InMemory(repo2)));

    assert!(old.is_some());
    assert_eq!(old.unwrap().name, "test");
    assert_eq!(manager.repo_count(), 1);
    assert!(manager.has_repo("test"));
}

#[tokio::test]
async fn test_default_repo() {
    let mut manager = RepoManager::new();
    let repo = Arc::new(InMemoryRepo::new());

    manager.set_default(RepoConfig::new("default", BoxRepo::InMemory(repo)));
    assert!(manager.has_repo("default"));

    let retrieved = manager.get_default_config().unwrap();
    assert_eq!(retrieved.name, "default");
    assert_eq!(manager.repo_count(), 1);
}

#[tokio::test]
async fn test_get_or_create_default() {
    let mut manager = RepoManager::new();

    let config1 = manager.get_or_create_default().await;
    assert_eq!(manager.repo_count(), 1);
    assert!(manager.has_repo("default"));
    assert_eq!(config1.name, "default");

    let config2 = manager.get_or_create_default().await;
    assert_eq!(manager.repo_count(), 1);
    assert_eq!(config2.name, "default");
}

#[tokio::test]
async fn test_multiple_repos_different_names() {
    let mut manager = RepoManager::new();

    manager.add_repo(RepoConfig::new("production", BoxRepo::InMemory(Arc::new(InMemoryRepo::new()))));
    manager.add_repo(RepoConfig::new("staging", BoxRepo::InMemory(Arc::new(InMemoryRepo::new()))));
    manager.add_repo(RepoConfig::new("testing", BoxRepo::InMemory(Arc::new(InMemoryRepo::new()))));
    manager.add_repo(RepoConfig::new("cache", BoxRepo::InMemory(Arc::new(InMemoryRepo::new()))));

    assert_eq!(manager.repo_count(), 4);
    assert!(manager.has_repo("production"));
    assert!(manager.has_repo("staging"));
    assert!(manager.has_repo("testing"));
    assert!(manager.has_repo("cache"));

    let _ = manager.get_repo_config("production").unwrap();
    let _ = manager.get_repo_config("staging").unwrap();
    let _ = manager.get_repo_config("testing").unwrap();
    let _ = manager.get_repo_config("cache").unwrap();
}
