use crate::db::storage::storage_canopy::CanopyRepo;
use crate::db::storage::storage_fjall::FjallRepo;
use crate::db::storage::storage_in_memory::InMemoryRepo;
use crate::db::storage::storage_nebari::NebariRepo;
use crate::db::storage::storage_persy::PersyRepo;
use crate::db::storage::storage_redb::RedbRepo;
use crate::db::storage::storage_sled::SledRepo;
use crate::db::storage::types::{Repo, Store};
use crate::db::DbResult;
use std::sync::Arc;

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
