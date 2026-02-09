use crate::db::error::DbResult;
use crate::db::storage::types::Repo;
use std::sync::Arc;

#[derive(Clone)]
pub enum BoxRepo {
    InMemory(Arc<crate::db::storage::storage_in_memory::InMemoryRepo>),
}

#[async_trait::async_trait]
impl Repo for BoxRepo {
    async fn store_get<S>(&self, name: S) -> DbResult<Arc<dyn crate::db::storage::types::Store>>
    where
        S: AsRef<str> + Send,
    {
        match self {
            BoxRepo::InMemory(repo) => repo.store_get(name).await,
        }
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        match self {
            BoxRepo::InMemory(repo) => repo.store_delete(name).await,
        }
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        match self {
            BoxRepo::InMemory(repo) => repo.stores_list().await,
        }
    }
}

impl From<Arc<crate::db::storage::storage_in_memory::InMemoryRepo>> for BoxRepo {
    fn from(repo: Arc<crate::db::storage::storage_in_memory::InMemoryRepo>) -> Self {
        BoxRepo::InMemory(repo)
    }
}
