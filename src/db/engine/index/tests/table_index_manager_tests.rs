use crate::db::engine::index::index_definition::IndexDefinition;
use crate::db::engine::index::index_info::IndexInfo;
use crate::db::engine::index::index_info_item::IndexInfoItem;
use crate::db::engine::index::table_index_manager::TableIndexManager;
use crate::db::storage::storage_in_memory::InMemoryStore;
use crate::db::storage::types::Store;
use crate::types::record_id::RecordId;
use std::sync::Arc;

#[tokio::test]
async fn test_has_indexes_initially_false() {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    let manager = TableIndexManager::new(data_store, info_store)
        .await
        .unwrap();

    assert_eq!(manager.has_indexes(), false);
    assert_eq!(manager.has_unique_indexes(), false);
}

#[tokio::test]
async fn test_has_indexes_true_after_load() {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let indexes = IndexInfo::from_definitions(vec![index_def]);
    let indexes_key = RecordId::system("indexes").to_bytes();
    let bytes = bincode::serialize(&indexes).unwrap();
    info_store.set(indexes_key, bytes.into()).await.unwrap();

    let manager = TableIndexManager::new(data_store, info_store)
        .await
        .unwrap();

    assert_eq!(manager.has_indexes(), true);
    assert_eq!(manager.has_unique_indexes(), false);
}

#[tokio::test]
async fn test_has_unique_indexes_true_after_load() {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    let index_def = IndexDefinition::new(1002, vec![IndexInfoItem::new(vec![1])]);
    let indexes = IndexInfo::from_definitions(vec![index_def]);
    let indexes_unique_key = RecordId::system("indexes_unique").to_bytes();
    let bytes = bincode::serialize(&indexes).unwrap();
    info_store
        .set(indexes_unique_key, bytes.into())
        .await
        .unwrap();

    let manager = TableIndexManager::new(data_store, info_store)
        .await
        .unwrap();

    assert_eq!(manager.has_indexes(), false);
    assert_eq!(manager.has_unique_indexes(), true);
}
