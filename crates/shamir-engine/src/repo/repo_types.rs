use shamir_storage::storage_canopy::CanopyRepo;
use shamir_storage::storage_fjall::FjallRepo;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::storage_nebari::NebariRepo;
use shamir_storage::storage_persy::PersyRepo;
use shamir_storage::storage_redb::RedbRepo;
use shamir_storage::storage_sled::SledRepo;
use shamir_storage::types::{Repo, Store};
use shamir_storage::error::DbResult;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task;

#[derive(Clone)]
pub enum BoxRepo {
    InMemory(Arc<InMemoryRepo>),
    Sled(Arc<SledRepo>),
    Redb(Arc<RedbRepo>),
    Fjall(Arc<FjallRepo>),
    Nebari(Arc<NebariRepo>),
    Persy(Arc<PersyRepo>),
    Canopy(Arc<CanopyRepo>),
}

#[async_trait::async_trait]
impl Repo for BoxRepo {
    async fn store_get<S>(&self, name: S) -> DbResult<Arc<dyn Store>>
    where
        S: AsRef<str> + Send,
    {
        match self {
            BoxRepo::InMemory(repo) => repo.store_get(name).await,
            BoxRepo::Sled(repo) => repo.store_get(name).await,
            BoxRepo::Redb(repo) => repo.store_get(name).await,
            BoxRepo::Fjall(repo) => repo.store_get(name).await,
            BoxRepo::Nebari(repo) => repo.store_get(name).await,
            BoxRepo::Persy(repo) => repo.store_get(name).await,
            BoxRepo::Canopy(repo) => repo.store_get(name).await,
        }
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        match self {
            BoxRepo::InMemory(repo) => repo.store_delete(name).await,
            BoxRepo::Sled(repo) => repo.store_delete(name).await,
            BoxRepo::Redb(repo) => repo.store_delete(name).await,
            BoxRepo::Fjall(repo) => repo.store_delete(name).await,
            BoxRepo::Nebari(repo) => repo.store_delete(name).await,
            BoxRepo::Persy(repo) => repo.store_delete(name).await,
            BoxRepo::Canopy(repo) => repo.store_delete(name).await,
        }
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        match self {
            BoxRepo::InMemory(repo) => repo.stores_list().await,
            BoxRepo::Sled(repo) => repo.stores_list().await,
            BoxRepo::Redb(repo) => repo.stores_list().await,
            BoxRepo::Fjall(repo) => repo.stores_list().await,
            BoxRepo::Nebari(repo) => repo.stores_list().await,
            BoxRepo::Persy(repo) => repo.stores_list().await,
            BoxRepo::Canopy(repo) => repo.stores_list().await,
        }
    }
}

impl From<Arc<InMemoryRepo>> for BoxRepo {
    fn from(repo: Arc<InMemoryRepo>) -> Self {
        BoxRepo::InMemory(repo)
    }
}

impl From<Arc<SledRepo>> for BoxRepo {
    fn from(repo: Arc<SledRepo>) -> Self {
        BoxRepo::Sled(repo)
    }
}

impl From<Arc<RedbRepo>> for BoxRepo {
    fn from(repo: Arc<RedbRepo>) -> Self {
        BoxRepo::Redb(repo)
    }
}

impl From<Arc<FjallRepo>> for BoxRepo {
    fn from(repo: Arc<FjallRepo>) -> Self {
        BoxRepo::Fjall(repo)
    }
}

impl From<Arc<NebariRepo>> for BoxRepo {
    fn from(repo: Arc<NebariRepo>) -> Self {
        BoxRepo::Nebari(repo)
    }
}

impl From<Arc<PersyRepo>> for BoxRepo {
    fn from(repo: Arc<PersyRepo>) -> Self {
        BoxRepo::Persy(repo)
    }
}

impl From<Arc<CanopyRepo>> for BoxRepo {
    fn from(repo: Arc<CanopyRepo>) -> Self {
        BoxRepo::Canopy(repo)
    }
}

// ============================================================================
// RepoFactory trait for async repo creation
// ============================================================================

/// Factory trait for asynchronously creating repositories.
/// Used to defer blocking file I/O operations to spawn_blocking.
#[async_trait::async_trait]
pub trait RepoFactory: Send + Sync {
    /// Creates a new repository, performing any blocking I/O in a separate thread.
    async fn create(&self) -> DbResult<BoxRepo>;
}

// ============================================================================
// RepoFactory implementations for async repo creation
// ============================================================================

/// Factory for InMemoryRepo - no blocking I/O needed
pub struct InMemoryRepoFactory;

#[async_trait::async_trait]
impl RepoFactory for InMemoryRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        Ok(BoxRepo::InMemory(Arc::new(InMemoryRepo::new())))
    }
}

/// Factory for SledRepo - uses spawn_blocking for file I/O
pub struct SledRepoFactory {
    pub path: PathBuf,
}

#[async_trait::async_trait]
impl RepoFactory for SledRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        let path = self.path.clone();
        let repo = task::spawn_blocking(move || SledRepo::new(path))
            .await
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))??;
        Ok(BoxRepo::Sled(Arc::new(repo)))
    }
}

/// Factory for RedbRepo - uses spawn_blocking for file I/O
pub struct RedbRepoFactory {
    pub path: PathBuf,
}

#[async_trait::async_trait]
impl RepoFactory for RedbRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        let path = self.path.clone();
        let repo = task::spawn_blocking(move || RedbRepo::new(path))
            .await
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))??;
        Ok(BoxRepo::Redb(Arc::new(repo)))
    }
}

/// Factory for FjallRepo - uses spawn_blocking for file I/O
pub struct FjallRepoFactory {
    pub path: PathBuf,
}

#[async_trait::async_trait]
impl RepoFactory for FjallRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        let path = self.path.clone();
        let repo = task::spawn_blocking(move || FjallRepo::new(path))
            .await
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))??;
        Ok(BoxRepo::Fjall(Arc::new(repo)))
    }
}

/// Factory for NebariRepo - uses spawn_blocking for file I/O
pub struct NebariRepoFactory {
    pub path: PathBuf,
}

#[async_trait::async_trait]
impl RepoFactory for NebariRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        let path = self.path.clone();
        let repo = task::spawn_blocking(move || NebariRepo::new(path))
            .await
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))??;
        Ok(BoxRepo::Nebari(Arc::new(repo)))
    }
}

/// Factory for PersyRepo - uses spawn_blocking for file I/O
pub struct PersyRepoFactory {
    pub path: PathBuf,
}

#[async_trait::async_trait]
impl RepoFactory for PersyRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        let path = self.path.clone();
        let repo = task::spawn_blocking(move || PersyRepo::new(path))
            .await
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))??;
        Ok(BoxRepo::Persy(Arc::new(repo)))
    }
}

/// Factory for CanopyRepo - uses spawn_blocking for file I/O
pub struct CanopyRepoFactory {
    pub path: PathBuf,
}

#[async_trait::async_trait]
impl RepoFactory for CanopyRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        let path = self.path.clone();
        let repo = task::spawn_blocking(move || CanopyRepo::new(path))
            .await
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))??;
        Ok(BoxRepo::Canopy(Arc::new(repo)))
    }
}

// ============================================================================
// BoxRepoFactory - enum for type-erased factory
// ============================================================================

/// Type-erased factory that can create any repo type
pub enum BoxRepoFactory {
    InMemory(InMemoryRepoFactory),
    Sled(SledRepoFactory),
    Redb(RedbRepoFactory),
    Fjall(FjallRepoFactory),
    Nebari(NebariRepoFactory),
    Persy(PersyRepoFactory),
    Canopy(CanopyRepoFactory),
}

impl BoxRepoFactory {
    pub fn in_memory() -> Self {
        BoxRepoFactory::InMemory(InMemoryRepoFactory)
    }

    pub fn sled(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Sled(SledRepoFactory { path: path.into() })
    }

    pub fn redb(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Redb(RedbRepoFactory { path: path.into() })
    }

    pub fn fjall(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Fjall(FjallRepoFactory { path: path.into() })
    }

    pub fn nebari(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Nebari(NebariRepoFactory { path: path.into() })
    }

    pub fn persy(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Persy(PersyRepoFactory { path: path.into() })
    }

    pub fn canopy(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Canopy(CanopyRepoFactory { path: path.into() })
    }
}

#[async_trait::async_trait]
impl RepoFactory for BoxRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        match self {
            BoxRepoFactory::InMemory(f) => f.create().await,
            BoxRepoFactory::Sled(f) => f.create().await,
            BoxRepoFactory::Redb(f) => f.create().await,
            BoxRepoFactory::Fjall(f) => f.create().await,
            BoxRepoFactory::Nebari(f) => f.create().await,
            BoxRepoFactory::Persy(f) => f.create().await,
            BoxRepoFactory::Canopy(f) => f.create().await,
        }
    }
}

impl Clone for BoxRepoFactory {
    fn clone(&self) -> Self {
        match self {
            BoxRepoFactory::InMemory(_) => BoxRepoFactory::in_memory(),
            BoxRepoFactory::Sled(f) => BoxRepoFactory::sled(f.path.clone()),
            BoxRepoFactory::Redb(f) => BoxRepoFactory::redb(f.path.clone()),
            BoxRepoFactory::Fjall(f) => BoxRepoFactory::fjall(f.path.clone()),
            BoxRepoFactory::Nebari(f) => BoxRepoFactory::nebari(f.path.clone()),
            BoxRepoFactory::Persy(f) => BoxRepoFactory::persy(f.path.clone()),
            BoxRepoFactory::Canopy(f) => BoxRepoFactory::canopy(f.path.clone()),
        }
    }
}
