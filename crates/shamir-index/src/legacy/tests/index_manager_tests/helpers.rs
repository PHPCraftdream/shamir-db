use crate::legacy::index_manager::IndexManager;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

/// Creates a new IndexManager with in-memory stores
pub(super) fn create_manager() -> (Arc<dyn Store>, Arc<dyn Store>, IndexManager) {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let manager = futures::executor::block_on(IndexManager::new(
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    ))
    .unwrap();
    (data_store, info_store, manager)
}

/// Creates a test InnerValue::Map with given key-value pairs
pub(super) fn create_test_value(pairs: &[(u64, InnerValue)]) -> InnerValue {
    let mut map = new_map();
    for (key, value) in pairs {
        map.insert(InternerKey::new(*key), value.clone());
    }
    InnerValue::Map(map)
}

/// Creates a nested map value for testing path extraction
pub(super) fn create_nested_value(keys: &[u64], leaf_value: InnerValue) -> InnerValue {
    if keys.is_empty() {
        return leaf_value;
    }
    let mut map = new_map();
    map.insert(
        InternerKey::new(keys[0]),
        create_nested_value(&keys[1..], leaf_value),
    );
    InnerValue::Map(map)
}
