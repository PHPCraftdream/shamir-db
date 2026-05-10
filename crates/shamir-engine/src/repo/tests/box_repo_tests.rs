use crate::repo::repo_types::BoxRepo;
use shamir_storage::storage_canopy::CanopyRepo;
use shamir_storage::storage_fjall::FjallRepo;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::storage_nebari::NebariRepo;
use shamir_storage::storage_persy::PersyRepo;
use shamir_storage::storage_redb::RedbRepo;
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
// Redb tests
// ============================================================================

#[tokio::test]
async fn test_box_repo_redb() {
    let temp_dir = tempfile::tempdir().unwrap();
    let redb_repo = RedbRepo::new(temp_dir.path().join("db.redb")).unwrap();
    let repo = BoxRepo::Redb(Arc::new(redb_repo));

    let _store = repo.store_get("test_table").await.unwrap();
    let stores = repo.stores_list().await.unwrap();
    assert!(stores.contains(&"test_table".to_string()));
}

#[test]
fn test_box_repo_from_redb() {
    let temp_dir = tempfile::tempdir().unwrap();
    let redb_repo = RedbRepo::new(temp_dir.path().join("db.redb")).unwrap();
    let repo: BoxRepo = Arc::new(redb_repo).into();
    assert!(matches!(repo, BoxRepo::Redb(_)));
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
// Nebari tests
// ============================================================================

#[tokio::test]
async fn test_box_repo_nebari() {
    let temp_dir = tempfile::tempdir().unwrap();
    let nebari_repo = NebariRepo::new(temp_dir.path()).unwrap();
    let repo = BoxRepo::Nebari(Arc::new(nebari_repo));

    let _store = repo.store_get("test_table").await.unwrap();
    let stores = repo.stores_list().await.unwrap();
    assert!(stores.contains(&"test_table".to_string()));
}

#[test]
fn test_box_repo_from_nebari() {
    let temp_dir = tempfile::tempdir().unwrap();
    let nebari_repo = NebariRepo::new(temp_dir.path()).unwrap();
    let repo: BoxRepo = Arc::new(nebari_repo).into();
    assert!(matches!(repo, BoxRepo::Nebari(_)));
}

// ============================================================================
// Persy tests
// ============================================================================

#[tokio::test]
async fn test_box_repo_persy() {
    let temp_dir = tempfile::tempdir().unwrap();
    let persy_repo = PersyRepo::new(temp_dir.path().join("test.persy")).unwrap();
    let repo = BoxRepo::Persy(Arc::new(persy_repo));

    let _store = repo.store_get("test_table").await.unwrap();
    let stores = repo.stores_list().await.unwrap();
    assert!(stores.contains(&"test_table".to_string()));
}

#[test]
fn test_box_repo_from_persy() {
    let temp_dir = tempfile::tempdir().unwrap();
    let persy_repo = PersyRepo::new(temp_dir.path().join("test.persy")).unwrap();
    let repo: BoxRepo = Arc::new(persy_repo).into();
    assert!(matches!(repo, BoxRepo::Persy(_)));
}

// ============================================================================
// Canopy tests
// ============================================================================

#[tokio::test]
async fn test_box_repo_canopy() {
    let temp_dir = tempfile::tempdir().unwrap();
    let canopy_repo = CanopyRepo::new(temp_dir.path()).unwrap();
    let repo = BoxRepo::Canopy(Arc::new(canopy_repo));

    let _store = repo.store_get("test_table").await.unwrap();
    let stores = repo.stores_list().await.unwrap();
    assert!(stores.contains(&"test_table".to_string()));
}

#[test]
fn test_box_repo_from_canopy() {
    let temp_dir = tempfile::tempdir().unwrap();
    let canopy_repo = CanopyRepo::new(temp_dir.path()).unwrap();
    let repo: BoxRepo = Arc::new(canopy_repo).into();
    assert!(matches!(repo, BoxRepo::Canopy(_)));
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
