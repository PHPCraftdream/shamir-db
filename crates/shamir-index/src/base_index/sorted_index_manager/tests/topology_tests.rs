use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;

use super::super::SortedIndexManager;

#[tokio::test]
async fn sorted_managers_and_clones_share_cached_topology() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let first = SortedIndexManager::new(Arc::clone(&store)).await.unwrap();
    let second = SortedIndexManager::new(store).await.unwrap();
    let cloned = first.clone();
    let expected = shamir_numa::detect_cached();
    for manager in [&first, &second, &cloned] {
        assert!(Arc::ptr_eq(manager.indexes.topology(), &expected));
    }
}
