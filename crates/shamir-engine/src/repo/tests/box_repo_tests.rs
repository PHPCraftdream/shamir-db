use crate::repo::repo_types::BoxRepo;
use shamir_storage::storage_fjall::FjallRepo;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::storage_sled::SledRepo;
use shamir_storage::types::Repo;
use std::sync::Arc;

// ============================================================================
// InMemory tests
// ============================================================================

#[tokio::test]
async fn test_box_repo_in_memory() {
    let repo = BoxRepo::InMemory(Arc::new(InMemoryRepo::new()));
    let _store = repo.store_get("test_table").await.unwrap();
    let stores = repo.stores_list().await.unwrap();
    assert!(stores.contains(&"test_table".to_string()));
}

#[test]
fn test_box_repo_from_in_memory() {
    let repo: BoxRepo = Arc::new(InMemoryRepo::new()).into();
    assert!(matches!(repo, BoxRepo::InMemory(_)));
}

// ============================================================================
// Sled tests
// ============================================================================

#[tokio::test]
async fn test_box_repo_sled() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sled_repo = SledRepo::new(temp_dir.path()).unwrap();
    let repo = BoxRepo::Sled(Arc::new(sled_repo));

    let _store = repo.store_get("test_table").await.unwrap();
    let stores = repo.stores_list().await.unwrap();
    assert!(stores.contains(&"test_table".to_string()));
}

#[test]
fn test_box_repo_from_sled() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sled_repo = SledRepo::new(temp_dir.path()).unwrap();
    let repo: BoxRepo = Arc::new(sled_repo).into();
    assert!(matches!(repo, BoxRepo::Sled(_)));
}

// ============================================================================
// Fjall tests
// ============================================================================

#[tokio::test]
async fn test_box_repo_fjall() {
    let temp_dir = tempfile::tempdir().unwrap();
    let fjall_repo = FjallRepo::new(temp_dir.path()).unwrap();
    let repo = BoxRepo::Fjall(Arc::new(fjall_repo));

    let _store = repo.store_get("test_table").await.unwrap();
    let stores = repo.stores_list().await.unwrap();
    assert!(stores.contains(&"test_table".to_string()));
}

#[test]
fn test_box_repo_from_fjall() {
    let temp_dir = tempfile::tempdir().unwrap();
    let fjall_repo = FjallRepo::new(temp_dir.path()).unwrap();
    let repo: BoxRepo = Arc::new(fjall_repo).into();
    assert!(matches!(repo, BoxRepo::Fjall(_)));
}

// ============================================================================
// Clone test
// ============================================================================

#[test]
fn test_box_repo_clone() {
    let repo1 = BoxRepo::InMemory(Arc::new(InMemoryRepo::new()));
    let repo2 = repo1.clone();

    assert!(matches!(repo1, BoxRepo::InMemory(_)));
    assert!(matches!(repo2, BoxRepo::InMemory(_)));
}
