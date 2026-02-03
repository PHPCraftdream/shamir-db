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

    #[tokio::test]
    async fn test_concurrent_inserts() {
        let (table, _dir) = create_test_table().await.unwrap();

        let num_threads = 20;
        let records_per_thread = 10;
        let mut handles = vec![];

        for thread_id in 0..num_threads {
            let table_clone = table.clone();
            handles.push(tokio::spawn(async move {
                let mut ids = vec![];
                for i in 0..records_per_thread {
                    let mut data = new_map();
                    data.insert("thread".to_string(), UserValue::Int(thread_id));
                    data.insert("index".to_string(), UserValue::Int(i));
                    data.insert("name".to_string(), UserValue::Str(format!("User_{}_{}", thread_id, i)));
                    let value = UserValue::Map(data);
                    let id = table_clone.insert(&value).await.unwrap();
                    ids.push(id);
                }
                ids
            }));
        }

        // Collect all IDs
        let mut all_ids = vec![];
        for handle in handles {
            let ids = handle.await.unwrap();
            all_ids.extend(ids);
        }

        assert_eq!(all_ids.len(), (num_threads * records_per_thread) as usize);

        // Verify all records can be retrieved
        let count = table.count().await.unwrap();
        assert_eq!(count, (num_threads * records_per_thread) as usize);
    }

    #[tokio::test]
    async fn test_concurrent_insert_and_read() {
        let (table, _dir) = create_test_table().await.unwrap();

        let num_inserters = 10;
        let num_readers = 10;
        let mut handles = vec![];

        // Inserters
        for i in 0..num_inserters {
            let table_clone = table.clone();
            handles.push(tokio::spawn(async move {
                for j in 0..20 {
                    let mut data = new_map();
                    data.insert("key".to_string(), UserValue::Str(format!("value_{}_{}", i, j)));
                    data.insert("num".to_string(), UserValue::Int(i * 20 + j));
                    table_clone.insert(&UserValue::Map(data)).await.unwrap();
                }
            }));
        }

        // Readers (may read while inserts happen)
        for _ in 0..num_readers {
            let table_clone = table.clone();
            handles.push(tokio::spawn(async move {
                for _ in 0..10 {
                    let _ = table_clone.list().await;
                    let _ = table_clone.count().await;
                }
            }));
        }

        // Wait for all
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify final count
        let count = table.count().await.unwrap();
        assert_eq!(count, (num_inserters * 20) as usize);
    }

    #[tokio::test]
    async fn test_concurrent_same_keys_interning() {
        let (table, _dir) = create_test_table().await.unwrap();

        let num_threads = 50;
        let mut handles = vec![];

        // All threads insert records with the same keys
        for i in 0..num_threads {
            let table_clone = table.clone();
            handles.push(tokio::spawn(async move {
                for j in 0..10 {
                    let mut data = new_map();
                    // Same keys across all threads
                    data.insert("name".to_string(), UserValue::Str(format!("User_{}", i)));
                    data.insert("age".to_string(), UserValue::Int(i));
                    data.insert("email".to_string(), UserValue::Str(format!("user{}@test.com", i)));
                    data.insert("index".to_string(), UserValue::Int(j));
                    table_clone.insert(&UserValue::Map(data)).await.unwrap();
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all records are correct
        let records = table.list().await.unwrap();
        assert_eq!(records.len(), (num_threads * 10) as usize);

        // All records should have the same 3 keys (name, age, email, index)
        for (_id, value) in records {
            match value {
                UserValue::Map(m) => {
                    assert_eq!(m.len(), 4);
                    assert!(m.contains_key("name"));
                    assert!(m.contains_key("age"));
                    assert!(m.contains_key("email"));
                    assert!(m.contains_key("index"));
                }
                _ => panic!("Expected Map"),
            }
        }
    }

    #[tokio::test]
    async fn test_concurrent_updates() {
        let (table, _dir) = create_test_table().await.unwrap();

        // Insert initial record
        let mut data = new_map();
        data.insert("counter".to_string(), UserValue::Int(0));
        let id = table.insert(&UserValue::Map(data)).await.unwrap();

        let num_threads = 20;
        let mut handles = vec![];

        // All threads update the same record
        for _ in 0..num_threads {
            let table_clone = table.clone();
            handles.push(tokio::spawn(async move {
                for i in 0..5 {
                    let mut data = new_map();
                    data.insert("counter".to_string(), UserValue::Int(i));
                    data.insert("thread".to_string(), UserValue::Str("test".to_string()));
                    let _ = table_clone.update(id, &UserValue::Map(data)).await;
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Final record should exist
        let final_record = table.get(id).await.unwrap();
        match final_record {
            UserValue::Map(m) => {
                assert!(m.contains_key("counter"));
                assert!(m.contains_key("thread"));
            }
            _ => panic!("Expected Map"),
        }
    }

    #[tokio::test]
    async fn test_concurrent_clone_and_operations() {
        let (table, _dir) = create_test_table().await.unwrap();

        let num_threads = 30;
        let mut handles = vec![];

        for i in 0..num_threads {
            let table_clone = table.clone();
            handles.push(tokio::spawn(async move {
                // Each thread does different operations
                match i % 4 {
                    0 => {
                        // Insert
                        let mut data = new_map();
                        data.insert("op".to_string(), UserValue::Str("insert".to_string()));
                        data.insert("num".to_string(), UserValue::Int(i));
                        table_clone.insert(&UserValue::Map(data)).await.unwrap();
                    }
                    1 => {
                        // List
                        let _ = table_clone.list().await;
                    }
                    2 => {
                        // Count
                        let _ = table_clone.count().await;
                    }
                    3 => {
                        // Insert then get
                        let mut data = new_map();
                        data.insert("op".to_string(), UserValue::Str("insert_get".to_string()));
                        let id = table_clone.insert(&UserValue::Map(data)).await.unwrap();
                        let _ = table_clone.get(id).await;
                    }
                    _ => unreachable!(),
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Should have inserted records
        let count = table.count().await.unwrap();
        assert!(count > 0);
    }

    #[tokio::test]
    async fn test_table_lazy_interner_loading() {
        let (table, _dir) = create_test_table().await.unwrap();

        // Interner should not be loaded yet
        // We can't check this directly, but we can verify behavior

        // First insert triggers lazy load
        let mut data = new_map();
        data.insert("first_key".to_string(), UserValue::Str("test".to_string()));
        table.insert(&UserValue::Map(data)).await.unwrap();

        // Clone table - should share the same interner
        let table_clone = table.clone();

        // Use the clone - should use the same loaded interner
        let mut data2 = new_map();
        data2.insert("first_key".to_string(), UserValue::Str("test2".to_string()));
        data2.insert("second_key".to_string(), UserValue::Int(42));
        table_clone.insert(&UserValue::Map(data2)).await.unwrap();

        // Verify both records
        let records = table_clone.list().await.unwrap();
        assert_eq!(records.len(), 2);
    }

    #[tokio::test]
    async fn test_table_with_nested_structures() {
        let (table, _dir) = create_test_table().await.unwrap();

        // Complex nested structure
        let mut inner_map = new_map();
        inner_map.insert("x".to_string(), UserValue::Int(10));
        inner_map.insert("y".to_string(), UserValue::Str("nested".to_string()));

        let list = vec![
            UserValue::Int(1),
            UserValue::Str("hello".to_string()),
            UserValue::Map(inner_map.clone()),
        ];

        let mut data = new_map();
        data.insert("list_data".to_string(), UserValue::List(list.clone()));
        data.insert("map_data".to_string(), UserValue::Map(inner_map));

        let id = table.insert(&UserValue::Map(data)).await.unwrap();

        // Retrieve and verify
        let retrieved = table.get(id).await.unwrap();

        match retrieved {
            UserValue::Map(m) => {
                match m.get("list_data") {
                    Some(UserValue::List(l)) => {
                        assert_eq!(l.len(), 3);
                    }
                    _ => panic!("Expected list"),
                }
                match m.get("map_data") {
                    Some(UserValue::Map(inner)) => {
                        assert_eq!(inner.len(), 2);
                        assert_eq!(inner.get("x"), Some(&UserValue::Int(10)));
                    }
                    _ => panic!("Expected map"),
                }
            }
            _ => panic!("Expected Map"),
        }
    }

    #[tokio::test]
    async fn test_table_with_special_characters() {
        let (table, _dir) = create_test_table().await.unwrap();

        let special_keys = vec![
            "key with spaces",
            "key-with-dashes",
            "key_with_underscores",
            "key.with.dots",
            "key:with:colons",
            "ключ-русский",
            "🔑emoji-key",
        ];

        for key in &special_keys {
            let mut data = new_map();
            data.insert(key.to_string(), UserValue::Str("value".to_string()));
            table.insert(&UserValue::Map(data)).await.unwrap();
        }

        // Retrieve all and verify
        let records = table.list().await.unwrap();
        assert_eq!(records.len(), special_keys.len());

        for (_id, value) in records {
            match value {
                UserValue::Map(m) => {
                    assert!(m.len() == 1);
                    let key = m.keys().next().unwrap();
                    assert!(special_keys.contains(&key.as_str()));
                }
                _ => panic!("Expected Map"),
            }
        }
    }

    #[tokio::test]
    async fn test_concurrent_delete() {
        let (table, _dir) = create_test_table().await.unwrap();

        // Insert some records
        let mut ids = vec![];
        for i in 0..20 {
            let mut data = new_map();
            data.insert("id".to_string(), UserValue::Int(i));
            let id = table.insert(&UserValue::Map(data)).await.unwrap();
            ids.push(id);
        }

        // Delete concurrently
        let _num_threads = 10;
        let mut handles = vec![];
        for chunk in ids.chunks(2) {
            let table_clone = table.clone();
            let chunk_ids = chunk.to_vec();
            handles.push(tokio::spawn(async move {
                for id in chunk_ids {
                    table_clone.delete(id).await.unwrap();
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // All should be deleted
        let count = table.count().await.unwrap();
        assert_eq!(count, 0);
    }
}
