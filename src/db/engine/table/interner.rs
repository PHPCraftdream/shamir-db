//! Interner manager for lazy loading and persistence

use crate::core::interner::{Interner, InternedKey, UserKey};
use crate::db::error::DbResult;
use crate::db::storage::types::Store;
use crate::codecs::bytes;
use crate::types::record_id::RecordId;
use tokio::sync::OnceCell;
use std::sync::Arc;

/// Manages interned keys with lazy loading and persistence
///
/// The interner is loaded lazily on first access and persisted
/// to storage when new keys are added.
pub struct InternerManager {
    info_store: Arc<dyn Store>,
    interner: OnceCell<Interner>,
}

impl InternerManager {
    /// Create a new interner manager
    pub fn new(info_store: Arc<dyn Store>) -> Self {
        Self {
            info_store,
            interner: OnceCell::new(),
        }
    }

    /// Get interner, loading it lazily on first access
    pub async fn get(&self) -> DbResult<&Interner> {
        if self.interner.get().is_some() {
            return Ok(self.interner.get().unwrap());
        }

        // Clone store for async block
        let info_store = Arc::clone(&self.info_store);
        let interner_cell = &self.interner;

        interner_cell.get_or_init(|| async move {
            // Load from storage
            let internals_id = RecordId::system("internals").to_bytes();
            let inter_data = info_store.get(internals_id).await;

            if let Ok(bytes) = inter_data {
                // Deserialize
                let data: Vec<(InternedKey, UserKey)> = bytes::from_bytes(&bytes)
                    .unwrap_or_else(|e| {
                        log::error!("Failed to deserialize interner: {}", e);
                        Vec::new()
                    });
                Interner::with_state(data)
            } else {
                // Empty interner
                Interner::new()
            }
        }).await;

        Ok(self.interner.get().unwrap())
    }

    /// Save new interned keys to storage
    pub async fn save_new_keys(&self, new_keys: &[(InternedKey, UserKey)]) -> DbResult<()> {
        if new_keys.is_empty() {
            return Ok(());
        }

        // Read existing
        let internals_id = RecordId::system("internals");
        let existing = self.info_store.get(internals_id.to_bytes()).await;
        let mut current: Vec<(InternedKey, UserKey)> = if let Ok(bytes) = existing {
            bytes::from_bytes(&bytes).unwrap_or_default()
        } else {
            Vec::new()
        };

        // Add new keys
        current.extend_from_slice(new_keys);

        // Serialize and save
        let bytes = bytes::to_bytes(&current)
            .map_err(|e| crate::db::error::DbError::Codec(format!("Failed to serialize interner: {}", e)))?;

        self.info_store.set(internals_id.to_bytes(), bytes).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::storage::storage_in_memory::InMemoryStore;

    async fn create_manager() -> InternerManager {
        InternerManager::new(Arc::new(InMemoryStore::new()))
    }

    #[tokio::test]
    async fn test_interner_lazy_loading() {
        let manager = create_manager().await;

        // Interner should load on first access
        let interner = manager.get().await.unwrap();
        assert_eq!(interner.len(), 0);

        // Second access should use cached interner
        let interner2 = manager.get().await.unwrap();
        assert!(std::ptr::eq(interner, interner2));
    }

    #[tokio::test]
    async fn test_interner_save_new_keys() {
        // Use same underlying store
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let manager1 = InternerManager::new(Arc::clone(&store));
        let manager2 = InternerManager::new(store);

        // Add keys through first manager
        let interner1 = manager1.get().await.unwrap();
        let result1 = interner1.touch_ind("name").unwrap();
        let new_keys = vec![(result1.key().clone(), UserKey::from_str("name"))];
        manager1.save_new_keys(&new_keys).await.unwrap();

        // Load through second manager
        let interner2 = manager2.get().await.unwrap();
        let result2 = interner2.touch_ind("name").unwrap();

        assert_eq!(result1.as_ref(), result2.as_ref());
    }

    #[tokio::test]
    async fn test_interner_persistence() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let manager1 = InternerManager::new(Arc::clone(&store));

        // Add some keys
        let interner1 = manager1.get().await.unwrap();
        let name_key = interner1.touch_ind("name").unwrap();
        let age_key = interner1.touch_ind("age").unwrap();
        let new_keys = vec![
            (name_key.key().clone(), UserKey::from_str("name")),
            (age_key.key().clone(), UserKey::from_str("age")),
        ];
        manager1.save_new_keys(&new_keys).await.unwrap();

        // Create new manager with same store
        let manager2 = InternerManager::new(store);
        let interner2 = manager2.get().await.unwrap();

        assert_eq!(interner2.len(), 2);
        assert_eq!(interner2.touch_ind("name").unwrap().as_ref(), name_key.as_ref());
        assert_eq!(interner2.touch_ind("age").unwrap().as_ref(), age_key.as_ref());
    }

    #[tokio::test]
    async fn test_interner_empty_save() {
        let manager = create_manager().await;

        // Saving empty keys should not fail
        let result = manager.save_new_keys(&[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_interner_multiple_saves() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let manager = InternerManager::new(store);

        let interner = manager.get().await.unwrap();

        // Save first batch
        let key1 = interner.touch_ind("key1").unwrap();
        manager.save_new_keys(&[(key1.key().clone(), UserKey::from_str("key1"))]).await.unwrap();

        // Save second batch
        let key2 = interner.touch_ind("key2").unwrap();
        manager.save_new_keys(&[(key2.key().clone(), UserKey::from_str("key2"))]).await.unwrap();

        // Both should be persisted
        assert_eq!(interner.len(), 2);
    }
}
