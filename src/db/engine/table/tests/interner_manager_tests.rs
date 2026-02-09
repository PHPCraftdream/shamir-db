use crate::core::interner::UserKey;
use crate::db::engine::table::interner_manager::InternerManager;
use crate::db::storage::storage_in_memory::InMemoryStore;
use crate::db::storage::types::Store;
use std::sync::Arc;

async fn create_manager() -> InternerManager {
    InternerManager::new(Arc::new(InMemoryStore::new()))
}

#[tokio::test]
async fn test_interner_lazy_loading() {
    let manager = create_manager().await;

    let interner = manager.get().await.unwrap();
    assert_eq!(interner.len(), 0);

    let interner2 = manager.get().await.unwrap();
    assert!(std::ptr::eq(interner, interner2));
}

#[tokio::test]
async fn test_interner_save_new_keys() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let manager1 = InternerManager::new(Arc::clone(&store));
    let manager2 = InternerManager::new(store);

    let interner1 = manager1.get().await.unwrap();
    let result1 = interner1.touch_ind("name").unwrap();
    let new_keys = vec![(result1.key().clone(), UserKey::from_str("name"))];
    manager1.save_new_keys(&new_keys).await.unwrap();

    let interner2 = manager2.get().await.unwrap();
    let result2 = interner2.touch_ind("name").unwrap();

    assert_eq!(result1.as_ref(), result2.as_ref());
}

#[tokio::test]
async fn test_interner_persistence() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let manager1 = InternerManager::new(Arc::clone(&store));

    let interner1 = manager1.get().await.unwrap();
    let name_key = interner1.touch_ind("name").unwrap();
    let age_key = interner1.touch_ind("age").unwrap();
    let new_keys = vec![
        (name_key.key().clone(), UserKey::from_str("name")),
        (age_key.key().clone(), UserKey::from_str("age")),
    ];
    manager1.save_new_keys(&new_keys).await.unwrap();

    let manager2 = InternerManager::new(store);
    let interner2 = manager2.get().await.unwrap();

    assert_eq!(interner2.len(), 2);
    assert_eq!(
        interner2.touch_ind("name").unwrap().as_ref(),
        name_key.as_ref()
    );
    assert_eq!(
        interner2.touch_ind("age").unwrap().as_ref(),
        age_key.as_ref()
    );
}

#[tokio::test]
async fn test_interner_empty_save() {
    let manager = create_manager().await;

    let result = manager.save_new_keys(&[]).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_interner_multiple_saves() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let manager = InternerManager::new(store);

    let interner = manager.get().await.unwrap();

    let key1 = interner.touch_ind("key1").unwrap();
    manager
        .save_new_keys(&[(key1.key().clone(), UserKey::from_str("key1"))])
        .await
        .unwrap();

    let key2 = interner.touch_ind("key2").unwrap();
    manager
        .save_new_keys(&[(key2.key().clone(), UserKey::from_str("key2"))])
        .await
        .unwrap();

    assert_eq!(interner.len(), 2);
}
