//! Type-erased Repo / RepoFactory enums.
//!
//! Each backend variant is `#[cfg]`-gated behind the matching feature
//! flag passed through from `shamir-storage`. With the default
//! feature set every backend is on; embedded builds can disable
//! whichever ones they don't need (`default-features = false,
//! features = ["redb"]`).

#[cfg(feature = "canopy")]
use shamir_storage::storage_canopy::CanopyRepo;
#[cfg(feature = "fjall")]
use shamir_storage::storage_fjall::FjallRepo;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
#[cfg(feature = "nebari")]
use shamir_storage::storage_nebari::NebariRepo;
#[cfg(feature = "persy")]
use shamir_storage::storage_persy::PersyRepo;
#[cfg(feature = "redb")]
use shamir_storage::storage_redb::RedbRepo;
#[cfg(feature = "sled")]
use shamir_storage::storage_sled::SledRepo;
use shamir_storage::types::{Repo, Store};
use shamir_storage::error::DbResult;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task;

#[derive(Clone)]
pub enum BoxRepo {
    InMemory(Arc<InMemoryRepo>),
    #[cfg(feature = "sled")]
    Sled(Arc<SledRepo>),
    #[cfg(feature = "redb")]
    Redb(Arc<RedbRepo>),
    #[cfg(feature = "fjall")]
    Fjall(Arc<FjallRepo>),
    #[cfg(feature = "nebari")]
    Nebari(Arc<NebariRepo>),
    #[cfg(feature = "persy")]
    Persy(Arc<PersyRepo>),
    #[cfg(feature = "canopy")]
    Canopy(Arc<CanopyRepo>),
    /// Recursive wrapper — buffers reads/writes in memory in front
    /// of any other backend. Today a passthrough; gains a bounded
    /// LRU cache + background flusher in a follow-up.
    MemBuffer(Arc<MemBufferRepoComposite>),
}

/// Holds the inner backend Repo (as another `BoxRepo`) plus the
/// buffer config. Public so `MemBuffer` variants can be matched
/// from other modules.
pub struct MemBufferRepoComposite {
    pub inner: BoxRepo,
    pub config: MemBufferConfig,
}

#[async_trait::async_trait]
impl Repo for BoxRepo {
    async fn store_get<S>(&self, name: S) -> DbResult<Arc<dyn Store>>
    where
        S: AsRef<str> + Send,
    {
        match self {
            BoxRepo::InMemory(repo) => repo.store_get(name).await,
            #[cfg(feature = "sled")]
            BoxRepo::Sled(repo) => repo.store_get(name).await,
            #[cfg(feature = "redb")]
            BoxRepo::Redb(repo) => repo.store_get(name).await,
            #[cfg(feature = "fjall")]
            BoxRepo::Fjall(repo) => repo.store_get(name).await,
            #[cfg(feature = "nebari")]
            BoxRepo::Nebari(repo) => repo.store_get(name).await,
            #[cfg(feature = "persy")]
            BoxRepo::Persy(repo) => repo.store_get(name).await,
            #[cfg(feature = "canopy")]
            BoxRepo::Canopy(repo) => repo.store_get(name).await,
            BoxRepo::MemBuffer(c) => {
                let inner_store = c.inner.store_get(name).await?;
                Ok(Arc::new(MemBufferStore::new(
                    inner_store,
                    c.config.clone(),
                )))
            }
        }
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        match self {
            BoxRepo::InMemory(repo) => repo.store_delete(name).await,
            #[cfg(feature = "sled")]
            BoxRepo::Sled(repo) => repo.store_delete(name).await,
            #[cfg(feature = "redb")]
            BoxRepo::Redb(repo) => repo.store_delete(name).await,
            #[cfg(feature = "fjall")]
            BoxRepo::Fjall(repo) => repo.store_delete(name).await,
            #[cfg(feature = "nebari")]
            BoxRepo::Nebari(repo) => repo.store_delete(name).await,
            #[cfg(feature = "persy")]
            BoxRepo::Persy(repo) => repo.store_delete(name).await,
            #[cfg(feature = "canopy")]
            BoxRepo::Canopy(repo) => repo.store_delete(name).await,
            BoxRepo::MemBuffer(c) => c.inner.store_delete(name).await,
        }
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        match self {
            BoxRepo::InMemory(repo) => repo.stores_list().await,
            #[cfg(feature = "sled")]
            BoxRepo::Sled(repo) => repo.stores_list().await,
            #[cfg(feature = "redb")]
            BoxRepo::Redb(repo) => repo.stores_list().await,
            #[cfg(feature = "fjall")]
            BoxRepo::Fjall(repo) => repo.stores_list().await,
            #[cfg(feature = "nebari")]
            BoxRepo::Nebari(repo) => repo.stores_list().await,
            #[cfg(feature = "persy")]
            BoxRepo::Persy(repo) => repo.stores_list().await,
            #[cfg(feature = "canopy")]
            BoxRepo::Canopy(repo) => repo.stores_list().await,
            BoxRepo::MemBuffer(c) => c.inner.stores_list().await,
        }
    }
}

impl From<Arc<InMemoryRepo>> for BoxRepo {
    fn from(repo: Arc<InMemoryRepo>) -> Self {
        BoxRepo::InMemory(repo)
    }
}

#[cfg(feature = "sled")]
impl From<Arc<SledRepo>> for BoxRepo {
    fn from(repo: Arc<SledRepo>) -> Self {
        BoxRepo::Sled(repo)
    }
}

#[cfg(feature = "redb")]
impl From<Arc<RedbRepo>> for BoxRepo {
    fn from(repo: Arc<RedbRepo>) -> Self {
        BoxRepo::Redb(repo)
    }
}

#[cfg(feature = "fjall")]
impl From<Arc<FjallRepo>> for BoxRepo {
    fn from(repo: Arc<FjallRepo>) -> Self {
        BoxRepo::Fjall(repo)
    }
}

#[cfg(feature = "nebari")]
impl From<Arc<NebariRepo>> for BoxRepo {
    fn from(repo: Arc<NebariRepo>) -> Self {
        BoxRepo::Nebari(repo)
    }
}

#[cfg(feature = "persy")]
impl From<Arc<PersyRepo>> for BoxRepo {
    fn from(repo: Arc<PersyRepo>) -> Self {
        BoxRepo::Persy(repo)
    }
}

#[cfg(feature = "canopy")]
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

#[cfg(feature = "sled")]
pub struct SledRepoFactory {
    pub path: PathBuf,
}

#[cfg(feature = "sled")]
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

#[cfg(feature = "redb")]
pub struct RedbRepoFactory {
    pub path: PathBuf,
}

#[cfg(feature = "redb")]
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

#[cfg(feature = "fjall")]
pub struct FjallRepoFactory {
    pub path: PathBuf,
}

#[cfg(feature = "fjall")]
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

#[cfg(feature = "nebari")]
pub struct NebariRepoFactory {
    pub path: PathBuf,
}

#[cfg(feature = "nebari")]
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

#[cfg(feature = "persy")]
pub struct PersyRepoFactory {
    pub path: PathBuf,
}

#[cfg(feature = "persy")]
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

#[cfg(feature = "canopy")]
pub struct CanopyRepoFactory {
    pub path: PathBuf,
}

#[cfg(feature = "canopy")]
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
    #[cfg(feature = "sled")]
    Sled(SledRepoFactory),
    #[cfg(feature = "redb")]
    Redb(RedbRepoFactory),
    #[cfg(feature = "fjall")]
    Fjall(FjallRepoFactory),
    #[cfg(feature = "nebari")]
    Nebari(NebariRepoFactory),
    #[cfg(feature = "persy")]
    Persy(PersyRepoFactory),
    #[cfg(feature = "canopy")]
    Canopy(CanopyRepoFactory),
    /// Recursive wrapper — buffer (LRU cache + background flusher,
    /// today passthrough) on top of any other factory.
    MemBuffer(Box<MemBufferRepoFactory>),
}

pub struct MemBufferRepoFactory {
    pub inner: BoxRepoFactory,
    pub config: MemBufferConfig,
}

impl BoxRepoFactory {
    pub fn in_memory() -> Self {
        BoxRepoFactory::InMemory(InMemoryRepoFactory)
    }

    #[cfg(feature = "sled")]
    pub fn sled(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Sled(SledRepoFactory { path: path.into() })
    }

    #[cfg(feature = "redb")]
    pub fn redb(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Redb(RedbRepoFactory { path: path.into() })
    }

    #[cfg(feature = "fjall")]
    pub fn fjall(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Fjall(FjallRepoFactory { path: path.into() })
    }

    #[cfg(feature = "nebari")]
    pub fn nebari(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Nebari(NebariRepoFactory { path: path.into() })
    }

    #[cfg(feature = "persy")]
    pub fn persy(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Persy(PersyRepoFactory { path: path.into() })
    }

    #[cfg(feature = "canopy")]
    pub fn canopy(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Canopy(CanopyRepoFactory { path: path.into() })
    }

    /// Wrap an existing factory in the membuffer layer. Resulting
    /// repo stays the same shape as `inner` but every `store_get`
    /// returns a `MemBufferStore` wrapping the inner backend's
    /// store.
    pub fn membuffer(inner: BoxRepoFactory, config: MemBufferConfig) -> Self {
        BoxRepoFactory::MemBuffer(Box::new(MemBufferRepoFactory { inner, config }))
    }
}

#[async_trait::async_trait]
impl RepoFactory for BoxRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        match self {
            BoxRepoFactory::InMemory(f) => f.create().await,
            #[cfg(feature = "sled")]
            BoxRepoFactory::Sled(f) => f.create().await,
            #[cfg(feature = "redb")]
            BoxRepoFactory::Redb(f) => f.create().await,
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Fjall(f) => f.create().await,
            #[cfg(feature = "nebari")]
            BoxRepoFactory::Nebari(f) => f.create().await,
            #[cfg(feature = "persy")]
            BoxRepoFactory::Persy(f) => f.create().await,
            #[cfg(feature = "canopy")]
            BoxRepoFactory::Canopy(f) => f.create().await,
            BoxRepoFactory::MemBuffer(f) => {
                let inner_repo = f.inner.create().await?;
                Ok(BoxRepo::MemBuffer(Arc::new(MemBufferRepoComposite {
                    inner: inner_repo,
                    config: f.config.clone(),
                })))
            }
        }
    }
}

impl Clone for BoxRepoFactory {
    fn clone(&self) -> Self {
        match self {
            BoxRepoFactory::InMemory(_) => BoxRepoFactory::in_memory(),
            #[cfg(feature = "sled")]
            BoxRepoFactory::Sled(f) => BoxRepoFactory::sled(f.path.clone()),
            #[cfg(feature = "redb")]
            BoxRepoFactory::Redb(f) => BoxRepoFactory::redb(f.path.clone()),
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Fjall(f) => BoxRepoFactory::fjall(f.path.clone()),
            #[cfg(feature = "nebari")]
            BoxRepoFactory::Nebari(f) => BoxRepoFactory::nebari(f.path.clone()),
            #[cfg(feature = "persy")]
            BoxRepoFactory::Persy(f) => BoxRepoFactory::persy(f.path.clone()),
            #[cfg(feature = "canopy")]
            BoxRepoFactory::Canopy(f) => BoxRepoFactory::canopy(f.path.clone()),
            BoxRepoFactory::MemBuffer(f) => BoxRepoFactory::membuffer(
                f.inner.clone(),
                f.config.clone(),
            ),
        }
    }
}
