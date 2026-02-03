//! High-level table with interning
//!
//! Provides UserValue/InnerValue transformations and key interning.

use crate::core::interner::Interner;
use crate::core::transform;
use crate::db::error::{DbError, DbResult};
use crate::db::storage::types::{Repo, Store};
use crate::types::record_id::RecordId;
use crate::types::value::{InnerValue, UserValue};
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// High-level table with automatic key interning
pub struct Table<R: Repo> {
    repo: Arc<R>,
    table_name: String,
    data_store: Arc<dyn Store>,
    info_store: Arc<dyn Store>,
    interner: Arc<OnceCell<Interner>>,
}

impl<R: Repo> Clone for Table<R> {
    fn clone(&self) -> Self {
        Self {
            repo: Arc::clone(&self.repo),
            table_name: self.table_name.clone(),
            data_store: Arc::clone(&self.data_store),
            info_store: Arc::clone(&self.info_store),
            interner: Arc::clone(&self.interner),
        }
    }
}

impl<R: Repo> Table<R> {
    /// Create a new table
    pub async fn new(repo: Arc<R>, table_name: String) -> DbResult<Self> {
        // Get or create stores
        let data_store = repo.store_get(format!("__data__{}", table_name)).await?;
        let info_store = repo.store_get(format!("__info__{}", table_name)).await?;

        Ok(Self {
            repo,
            table_name,
            data_store,
            info_store,
            interner: Arc::new(OnceCell::new()),
        })
    }

    /// Get the interner, loading it lazily on first access
    async fn get_interner(&self) -> DbResult<&Interner> {
        if self.interner.get().is_some() {
            return Ok(self.interner.get().unwrap());
        }

        // Clone Arcs for async block
        let info_store = Arc::clone(&self.info_store);
        let interner_cell = Arc::clone(&self.interner);

        interner_cell.get_or_init(|| async move {
            // Load from info_store
            let internals_id = RecordId::system("internals");
            let inter_data = info_store.get(internals_id).await;

            if let Ok(bytes) = inter_data {
                // Deserialize: Vec<(u64, String)>
                let data: Vec<(u64, String)> = bincode::deserialize(&bytes)
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

    /// Save new interned keys to info_store
    async fn save_new_keys(&self, new_keys: &[(u64, String)]) -> DbResult<()> {
        if new_keys.is_empty() {
            return Ok(());
        }

        // Save the full interner state
        let internals_id = RecordId::system("internals");

        // Read existing
        let existing = self.info_store.get(internals_id).await;
        let mut current: Vec<(u64, String)> = if let Ok(bytes) = existing {
            bincode::deserialize(&bytes)
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        // Add new keys
        current.extend_from_slice(new_keys);

        // Serialize and save
        let bytes = bincode::serialize(&current)
            .map_err(|e| DbError::Codec(format!("Failed to serialize interner: {}", e)))?;

        self.info_store.set(internals_id, Bytes::from(bytes)).await?;

        Ok(())
    }

    /// Insert a UserValue, returns RecordId
    pub async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
        let interner = self.get_interner().await?;

        // Transform UserValue → InnerValue
        let transform = transform::user_to_inner(value, interner);

        // Save new keys if any
        if let Some(ref new_keys) = transform.new_keys {
            self.save_new_keys(new_keys).await?;
        }

        // Serialize InnerValue
        let inner_bytes = transform.inner_value.to_bytes();

        // Insert to data store
        self.data_store.insert(inner_bytes).await
    }

    /// Get a UserValue by RecordId
    pub async fn get(&self, id: RecordId) -> DbResult<UserValue> {
        let interner = self.get_interner().await?;

        // Read from data store
        let bytes = self.data_store.get(id).await?;

        // Deserialize InnerValue
        let inner_value = InnerValue::from_bytes(bytes)
            .map_err(|e| DbError::Codec(format!("Failed to deserialize InnerValue: {}", e)))?;

        // Transform InnerValue → UserValue
        Ok(transform::inner_to_user(&inner_value, interner))
    }

    /// Update a record by RecordId
    pub async fn update(&self, id: RecordId, value: &UserValue) -> DbResult<bool> {
        let interner = self.get_interner().await?;

        // Check if exists
        let exists = self.data_store.get(id).await.is_ok();
        if !exists {
            return Ok(false);
        }

        // Transform UserValue → InnerValue
        let transform = transform::user_to_inner(value, interner);

        // Save new keys if any
        if let Some(ref new_keys) = transform.new_keys {
            self.save_new_keys(new_keys).await?;
        }

        // Serialize and update
        let inner_bytes = transform.inner_value.to_bytes();
        self.data_store.set(id, inner_bytes).await?;
        Ok(true)  // Existed and updated
    }

    /// Delete a record by RecordId
    pub async fn delete(&self, id: RecordId) -> DbResult<bool> {
        self.data_store.remove(id).await
    }

    /// List all records
    pub async fn list(&self) -> DbResult<Vec<(RecordId, UserValue)>> {
        let interner = self.get_interner().await?;

        let items = self.data_store.iter().await?;
        let mut result = Vec::new();

        for (id, bytes) in items {
            match InnerValue::from_bytes(bytes) {
                Ok(inner_value) => {
                    let user_value = transform::inner_to_user(&inner_value, interner);
                    result.push((id, user_value));
                }
                Err(e) => {
                    log::warn!("Failed to deserialize record: {}", e);
                }
            }
        }

        Ok(result)
    }

    /// Count records
    pub async fn count(&self) -> DbResult<usize> {
        let items = self.data_store.iter().await?;
        Ok(items.len())
    }

    /// Get table name
    pub fn name(&self) -> &str {
        &self.table_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::storage::storage_sled::SledRepo;
    use crate::types::common::new_map;

    async fn create_test_table() -> DbResult<(Table<SledRepo>, tempfile::TempDir)> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test_db");
        let repo = Arc::new(SledRepo::new(path)?);
        let table = Table::new(repo, "users".to_string()).await?;

        Ok((table, dir))
    }

    #[tokio::test]
    async fn test_table_insert_and_get() {
        let (table, _dir) = create_test_table().await.unwrap();

        let mut user_data = new_map();
        user_data.insert("name".to_string(), UserValue::Str("Alice".to_string()));
        user_data.insert("age".to_string(), UserValue::Int(30));
        user_data.insert("email".to_string(), UserValue::Str("alice@example.com".to_string()));
        let user_value = UserValue::Map(user_data);

        let id = table.insert(&user_value).await.unwrap();

        let retrieved = table.get(id).await.unwrap();
        assert_eq!(retrieved, user_value);
    }

    #[tokio::test]
    async fn test_table_interning_persistence() {
        let (table, _dir) = create_test_table().await.unwrap();

        // Insert first record
        let mut data1 = new_map();
        data1.insert("name".to_string(), UserValue::Str("Bob".to_string()));
        let original1 = UserValue::Map(data1.clone());
        let id1 = table.insert(&original1).await.unwrap();

        // Insert second record with overlapping keys
        let mut data2 = new_map();
        data2.insert("name".to_string(), UserValue::Str("Charlie".to_string()));
        data2.insert("age".to_string(), UserValue::Int(25));
        let id2 = table.insert(&UserValue::Map(data2)).await.unwrap();

        // Verify both records
        let retrieved1 = table.get(id1).await.unwrap();
        assert_eq!(retrieved1, original1);

        let retrieved2 = table.get(id2).await.unwrap();
        // Check it has the right data
        match retrieved2 {
            UserValue::Map(m) => {
                assert_eq!(m.get("name"), Some(&UserValue::Str("Charlie".to_string())));
                assert_eq!(m.get("age"), Some(&UserValue::Int(25)));
            }
            _ => panic!("Expected Map"),
        }
    }

    #[tokio::test]
    async fn test_table_update() {
        let (table, _dir) = create_test_table().await.unwrap();

        let mut data = new_map();
        data.insert("name".to_string(), UserValue::Str("Dave".to_string()));
        let id = table.insert(&UserValue::Map(data.clone())).await.unwrap();

        // Update
        let mut updated = new_map();
        updated.insert("name".to_string(), UserValue::Str("David".to_string()));
        updated.insert("age".to_string(), UserValue::Int(40));

        let existed = table.update(id, &UserValue::Map(updated)).await.unwrap();
        assert!(existed);

        let retrieved = table.get(id).await.unwrap();
        match retrieved {
            UserValue::Map(m) => {
                assert_eq!(m.get("name"), Some(&UserValue::Str("David".to_string())));
                assert_eq!(m.get("age"), Some(&UserValue::Int(40)));
            }
            _ => panic!("Expected Map"),
        }
    }

    #[tokio::test]
    async fn test_table_delete() {
        let (table, _dir) = create_test_table().await.unwrap();

        let mut data = new_map();
        data.insert("name".to_string(), UserValue::Str("Eve".to_string()));
        let id = table.insert(&UserValue::Map(data)).await.unwrap();

        let deleted = table.delete(id).await.unwrap();
        assert!(deleted);

        let get_result = table.get(id).await;
        assert!(matches!(get_result, Err(DbError::NotFound(_))));

        let deleted_again = table.delete(id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_table_list() {
        let (table, _dir) = create_test_table().await.unwrap();

        for i in 1..=3 {
            let mut data = new_map();
            data.insert("id".to_string(), UserValue::Int(i));
            data.insert("name".to_string(), UserValue::Str(format!("User{}", i)));
            table.insert(&UserValue::Map(data)).await.unwrap();
        }

        let records = table.list().await.unwrap();
        assert_eq!(records.len(), 3);
    }

    #[tokio::test]
    async fn test_table_count() {
        let (table, _dir) = create_test_table().await.unwrap();

        assert_eq!(table.count().await.unwrap(), 0);

        for i in 1..=5 {
            let mut data = new_map();
            data.insert("id".to_string(), UserValue::Int(i));
            table.insert(&UserValue::Map(data)).await.unwrap();
        }

        assert_eq!(table.count().await.unwrap(), 5);
    }
}
