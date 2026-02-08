use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{OnceCell, RwLock};
use crate::core::interner::Interner;
use crate::db::engine::index::index_info::IndexInfo;
use crate::db::storage::types::Store;
use crate::types::record_id::RecordId;

pub struct TableIndexManager {
    _data_store: Arc<dyn Store>,

    _interner: Arc<OnceCell<Interner>>,

    _indexes: Arc<RwLock<IndexInfo>>,

    _indexes_unique: Arc<RwLock<IndexInfo>>,

    has_indexes: AtomicBool,

    has_indexes_unique: AtomicBool,

    _info_store: Arc<dyn Store>,
}

impl TableIndexManager {
    pub async fn new(
        data_store: Arc<dyn Store>,
        info_store: Arc<dyn Store>,
        interner: Arc<OnceCell<Interner>>,
    ) -> Result<Self, crate::db::error::DbError> {
        let indexes_key = RecordId::system("indexes").to_bytes();
        let indexes_unique_key = RecordId::system("indexes_unique").to_bytes();

        let indexes = match info_store.get(indexes_key.clone()).await {
            Ok(bytes) => bincode::deserialize::<IndexInfo>(&bytes)
                .unwrap_or_else(|_| IndexInfo::disabled()),
            Err(crate::db::error::DbError::NotFound(_)) => IndexInfo::disabled(),
            Err(e) => return Err(e),
        };

        let indexes_unique = match info_store.get(indexes_unique_key.clone()).await {
            Ok(bytes) => bincode::deserialize::<IndexInfo>(&bytes)
                .unwrap_or_else(|_| IndexInfo::disabled()),
            Err(crate::db::error::DbError::NotFound(_)) => IndexInfo::disabled(),
            Err(e) => return Err(e),
        };

        let has_indexes = AtomicBool::new(indexes.is_enabled());
        let has_indexes_unique = AtomicBool::new(indexes_unique.is_enabled());

        Ok(Self {
            _data_store: data_store,
            _interner: interner,
            _indexes: Arc::new(RwLock::new(indexes)),
            _indexes_unique: Arc::new(RwLock::new(indexes_unique)),
            has_indexes,
            has_indexes_unique,
            _info_store: info_store,
        })
    }

    pub fn has_indexes(&self) -> bool {
        self.has_indexes.load(Ordering::Relaxed)
    }

    pub fn has_unique_indexes(&self) -> bool {
        self.has_indexes_unique.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::storage::storage_in_memory::InMemoryStore;
    use crate::db::engine::index::index_definition::IndexDefinition;
    use crate::db::engine::index::index_info_item::IndexInfoItem;

    #[tokio::test]
    async fn test_has_indexes_initially_false() {
        let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let interner = Arc::new(OnceCell::new());

        let manager = TableIndexManager::new(data_store, info_store, interner)
            .await
            .unwrap();

        assert_eq!(manager.has_indexes(), false);
        assert_eq!(manager.has_unique_indexes(), false);
    }

    #[tokio::test]
    async fn test_has_indexes_true_after_load() {
        let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let interner = Arc::new(OnceCell::new());

        let indexes = IndexInfo::all();
        let indexes_key = RecordId::system("indexes").to_bytes();
        let bytes = bincode::serialize(&indexes).unwrap();
        info_store.set(indexes_key, bytes.into()).await.unwrap();

        let manager = TableIndexManager::new(data_store, info_store, interner)
            .await
            .unwrap();

        assert_eq!(manager.has_indexes(), true);
        assert_eq!(manager.has_unique_indexes(), false);
    }

    #[tokio::test]
    async fn test_has_unique_indexes_true_after_load() {
        let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let interner = Arc::new(OnceCell::new());

        let index_def = IndexDefinition::new("unique_email", vec![IndexInfoItem::new(vec![1])]);
        let indexes = IndexInfo::selective(vec![index_def]);
        let indexes_unique_key = RecordId::system("indexes_unique").to_bytes();
        let bytes = bincode::serialize(&indexes).unwrap();
        info_store.set(indexes_unique_key, bytes.into()).await.unwrap();

        let manager = TableIndexManager::new(data_store, info_store, interner)
            .await
            .unwrap();

        assert_eq!(manager.has_indexes(), false);
        assert_eq!(manager.has_unique_indexes(), true);
    }
}
