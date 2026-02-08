//! High-level table with interning
//!
//! Provides UserValue/InnerValue transformations and key interning.

use crate::core::interner::{Interner, InternedKey, UserKey};
use crate::core::transform;
use crate::db::error::{DbError, DbResult};
use crate::db::storage::types::{Repo, Store};
use crate::types::record_id::RecordId;
use crate::types::value::{InnerValue, UserValue};
use crate::codecs::bytes;
use async_stream::stream;
use futures::stream::{Stream, StreamExt};
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell};

/// Get the system record key for storing record count
fn count_key() -> RecordId {
    RecordId::system("count")
}

/// High-level table with automatic key interning
pub struct Table<R: Repo> {
    repo: Arc<R>,
    table_name: String,
    data_store: Arc<dyn Store>,
    info_store: Arc<dyn Store>,
    interner: Arc<OnceCell<Interner>>,
    batch_size: usize,
    /// Mutex for synchronizing counter updates
    counter_mutex: Arc<Mutex<()>>,
}

impl<R: Repo> Clone for Table<R> {
    fn clone(&self) -> Self {
        Self {
            repo: Arc::clone(&self.repo),
            table_name: self.table_name.clone(),
            data_store: Arc::clone(&self.data_store),
            info_store: Arc::clone(&self.info_store),
            interner: Arc::clone(&self.interner),
            batch_size: self.batch_size,
            counter_mutex: Arc::clone(&self.counter_mutex),
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
            batch_size: 1000, // default batch size
            counter_mutex: Arc::new(Mutex::new(())),
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
            // Load from info_store - convert RecordId to Bytes
            let internals_id = RecordId::system("internals").to_bytes();
            let inter_data = info_store.get(internals_id).await;

            if let Ok(bytes) = inter_data {
                // Deserialize: Vec<(InternedKey, UserKey)>
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

    /// Save new interned keys to info_store
    async fn save_new_keys(&self, new_keys: &[(InternedKey, UserKey)]) -> DbResult<()> {
        if new_keys.is_empty() {
            return Ok(());
        }

        // Save the full interner state - convert RecordId to Bytes
        let internals_id = RecordId::system("internals");

        // Read existing
        let existing = self.info_store.get(internals_id.to_bytes()).await;
        let mut current: Vec<(InternedKey, UserKey)> = if let Ok(bytes) = existing {
            bytes::from_bytes(&bytes)
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        // Add new keys
        current.extend_from_slice(new_keys);

        // Serialize and save
        let bytes = bytes::to_bytes(&current)
            .map_err(|e| DbError::Codec(format!("Failed to serialize interner: {}", e)))?;

        self.info_store.set(internals_id.to_bytes(), bytes).await?;

        Ok(())
    }

    /// Get the current record count from the counter
    async fn get_record_count(&self) -> DbResult<u64> {
        let key_bytes = count_key().to_bytes();
        match self.info_store.get(key_bytes).await {
            Ok(bytes) => {
                // Deserialize u64
                let count: u64 = bytes::from_bytes(&bytes)
                    .map_err(|e| DbError::Codec(format!("Failed to deserialize count: {}", e)))?;
                Ok(count)
            }
            Err(DbError::NotFound(_)) => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Set the record count (useful for initialization or manual correction)
    async fn set_record_count(&self, count: u64) -> DbResult<()> {
        let key_bytes = count_key().to_bytes();
        let bytes = bytes::to_bytes(&count)
            .map_err(|e| DbError::Codec(format!("Failed to serialize count: {}", e)))?;
        self.info_store.set(key_bytes, bytes).await?;
        Ok(())
    }

    /// Increment the record count by delta (with mutex lock for thread safety)
    async fn increment_record_count(&self, delta: i64) -> DbResult<()> {
        let _guard = self.counter_mutex.lock().await;
        let current = self.get_record_count().await? as i64;
        let new_count = current + delta;
        if new_count < 0 {
            return Err(DbError::Internal(format!(
                "Record count cannot be negative: current={}, delta={}",
                current, delta
            )));
        }
        self.set_record_count(new_count as u64).await
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

        // Insert to data store - returns Bytes (16 random bytes)
        let key_bytes = self.data_store.insert(inner_bytes).await?;

        // Increment record count
        self.increment_record_count(1).await?;

        // Convert Bytes to RecordId
        let arr: [u8; 16] = key_bytes.as_ref().try_into()
            .map_err(|_| DbError::Internal("Failed to convert key bytes to RecordId".to_string()))?;
        Ok(RecordId(arr))
    }

    /// Get a UserValue by RecordId
    pub async fn get(&self, id: RecordId) -> DbResult<UserValue> {
        let interner = self.get_interner().await?;

        // Convert RecordId to Bytes
        let key_bytes = id.to_bytes();

        // Read from data store
        let bytes = self.data_store.get(key_bytes).await?;

        // Deserialize InnerValue
        let inner_value = InnerValue::from_bytes(bytes)
            .map_err(|e| DbError::Codec(format!("Failed to deserialize InnerValue: {}", e)))?;

        // Transform InnerValue → UserValue
        Ok(transform::inner_to_user(&inner_value, interner))
    }

    /// Update a record by RecordId
    pub async fn update(&self, id: RecordId, value: &UserValue) -> DbResult<bool> {
        let interner = self.get_interner().await?;

        // Convert RecordId to Bytes
        let key_bytes = id.to_bytes();

        // Check if exists
        let exists = self.data_store.get(key_bytes.clone()).await.is_ok();
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
        self.data_store.set(key_bytes, inner_bytes).await?;
        Ok(true)  // Existed and updated
    }

    /// Set a record by RecordId - creates if not exists, updates if exists
    /// Returns true if created, false if updated
    pub async fn set(&self, id: RecordId, value: &UserValue) -> DbResult<bool> {
        let interner = self.get_interner().await?;

        // Convert RecordId to Bytes
        let key_bytes = id.to_bytes();

        // Check if exists
        let exists = self.data_store.get(key_bytes.clone()).await.is_ok();

        // Transform UserValue → InnerValue
        let transform = transform::user_to_inner(value, interner);

        // Save new keys if any
        if let Some(ref new_keys) = transform.new_keys {
            self.save_new_keys(new_keys).await?;
        }

        // Serialize and set
        let inner_bytes = transform.inner_value.to_bytes();
        self.data_store.set(key_bytes, inner_bytes).await?;

        if !exists {
            // New record created - increment count
            self.increment_record_count(1).await?;
        }

        Ok(!exists)  // true if created, false if updated
    }

    /// Delete a record by RecordId
    pub async fn delete(&self, id: RecordId) -> DbResult<bool> {
        // Convert RecordId to Bytes
        let key_bytes = id.to_bytes();
        let removed = self.data_store.remove(key_bytes).await?;

        if removed {
            // Decrement record count
            self.increment_record_count(-1).await?;
        }

        Ok(removed)
    }

    /// List all records
    pub async fn list(&self) -> DbResult<Vec<(RecordId, UserValue)>> {
        let interner = self.get_interner().await?;

        let items = self.data_store.iter().await?;
        let mut result = Vec::new();

        for (key_bytes, bytes) in items {
            // Convert Bytes to RecordId
            let arr: [u8; 16] = key_bytes.as_ref().try_into()
                .map_err(|_| DbError::Internal("Failed to convert key bytes to RecordId".to_string()))?;
            let id = RecordId(arr);

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

    /// Count records (uses the stored counter for O(1) performance)
    pub async fn count(&self) -> DbResult<usize> {
        Ok(self.get_record_count().await? as usize)
    }

    /// Set batch size for streaming operations
    pub fn set_batch_size(&mut self, size: usize) {
        self.batch_size = size;
    }

    /// Stream records in batches, returning UserValues
    ///
    /// This is memory-efficient for large tables as it doesn't load all records at once.
    /// Returns a stream that yields batches of records.
    ///
    /// # Arguments
    /// * `batch_size` - Number of records per batch
    ///
    /// # Returns
    /// A stream that yields batches of (RecordId, UserValue) tuples
    ///
    /// Stream all records in batches (async generator like PHP)
    pub fn list_stream(
        &self,
        batch_size: usize,
    ) -> impl Stream<Item = DbResult<Vec<(RecordId, UserValue)>>> {
        let table = self.clone();

        stream! {
            // Get interner once
            let interner = table.get_interner().await?;

            // Get stream from storage
            let mut storage_stream = table.data_store.iter_stream(batch_size);

            // Transform each batch
            while let Some(batch_result) = storage_stream.next().await {
                let batch_bytes = batch_result?;

                // Transform batch
                let mut batch = Vec::new();
                for (key_bytes, bytes) in batch_bytes {
                    // Convert Bytes to RecordId
                    let arr: [u8; 16] = match key_bytes.as_ref().try_into() {
                        Ok(a) => a,
                        Err(_) => {
                            yield Err(DbError::Internal("Failed to convert key bytes to RecordId".to_string()));
                            continue;
                        }
                    };
                    let id = RecordId(arr);

                    match InnerValue::from_bytes(bytes) {
                        Ok(inner_value) => {
                            let user_value = transform::inner_to_user(&inner_value, interner);
                            batch.push((id, user_value));
                        }
                        Err(e) => {
                            yield Err(DbError::Codec(format!("Failed to deserialize record: {}", e)));
                        }
                    }
                }

                if !batch.is_empty() {
                    yield Ok(batch);
                }
            }
        }
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
                    assert_eq!(m.len(), 1);
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

    #[tokio::test]
    async fn test_interner_persistence_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_persistence_db");
        let table_name = "users";

        // === First session: write data ===
        let repo1 = Arc::new(SledRepo::new(path.clone()).unwrap());
        let table1 = Table::new(repo1.clone(), table_name.to_string()).await.unwrap();

        // Insert multiple records with overlapping keys to test interning
        let mut data1 = new_map();
        data1.insert("name".to_string(), UserValue::Str("Alice".to_string()));
        data1.insert("email".to_string(), UserValue::Str("alice@example.com".to_string()));
        data1.insert("age".to_string(), UserValue::Int(30));
        let value1 = UserValue::Map(data1);

        let id1 = table1.insert(&value1).await.unwrap();

        // Insert second record with same keys (should reuse interner entries)
        let mut data2 = new_map();
        data2.insert("name".to_string(), UserValue::Str("Bob".to_string()));
        data2.insert("email".to_string(), UserValue::Str("bob@example.com".to_string()));
        data2.insert("age".to_string(), UserValue::Int(25));
        let value2 = UserValue::Map(data2);

        let id2 = table1.insert(&value2).await.unwrap();

        // Verify records in first session
        let retrieved1 = table1.get(id1).await.unwrap();
        assert_eq!(retrieved1, value1);

        let retrieved2 = table1.get(id2).await.unwrap();
        assert_eq!(retrieved2, value2);

        let count1 = table1.count().await.unwrap();
        assert_eq!(count1, 2);

        // table1 and repo1 are dropped here, closing the database
        drop(table1);
        drop(repo1);

        // === Second session: reopen and verify ===
        let repo2 = Arc::new(SledRepo::new(path).unwrap());
        let table2 = Table::new(repo2, table_name.to_string()).await.unwrap();

        // Verify records are still there after restart
        let retrieved1_after = table2.get(id1).await.unwrap();
        assert_eq!(retrieved1_after, value1, "First record should match after restart");

        let retrieved2_after = table2.get(id2).await.unwrap();
        assert_eq!(retrieved2_after, value2, "Second record should match after restart");

        // Verify count
        let count2 = table2.count().await.unwrap();
        assert_eq!(count2, 2, "Should have 2 records after restart");

        // Insert new record with same keys (should reuse restored interner entries)
        let mut data3 = new_map();
        data3.insert("name".to_string(), UserValue::Str("Charlie".to_string()));
        data3.insert("email".to_string(), UserValue::Str("charlie@example.com".to_string()));
        data3.insert("age".to_string(), UserValue::Int(35));
        let value3 = UserValue::Map(data3);

        let id3 = table2.insert(&value3).await.unwrap();

        // Verify all three records
        let retrieved3 = table2.get(id3).await.unwrap();
        assert_eq!(retrieved3, value3);

        let count3 = table2.count().await.unwrap();
        assert_eq!(count3, 3, "Should have 3 records after inserting in second session");

        // List all records and verify
        let all_records = table2.list().await.unwrap();
        assert_eq!(all_records.len(), 3);

        // Verify each record has the correct structure
        for (_id, value) in all_records {
            match value {
                UserValue::Map(m) => {
                    assert!(m.contains_key("name"), "Should have 'name' key");
                    assert!(m.contains_key("email"), "Should have 'email' key");
                    assert!(m.contains_key("age"), "Should have 'age' key");
                    assert_eq!(m.len(), 3, "Should have exactly 3 keys");
                }
                _ => panic!("Expected Map"),
            }
        }
    }

    #[tokio::test]
    async fn test_set_method_creates_new_record() {
        let (table, _dir) = create_test_table().await.unwrap();

        // Create a new RecordId
        let id = RecordId::new();

        let mut data = new_map();
        data.insert("name".to_string(), UserValue::Str("Alice".to_string()));
        data.insert("age".to_string(), UserValue::Int(30));
        let value = UserValue::Map(data);

        // set should create new record
        let created = table.set(id, &value).await.unwrap();
        assert!(created, "Should return true for new record");

        // Verify count increased
        assert_eq!(table.count().await.unwrap(), 1);

        // Verify record exists
        let retrieved = table.get(id).await.unwrap();
        assert_eq!(retrieved, value);
    }

    #[tokio::test]
    async fn test_set_method_updates_existing_record() {
        let (table, _dir) = create_test_table().await.unwrap();

        // First insert a record
        let id = RecordId::new();
        let mut data1 = new_map();
        data1.insert("name".to_string(), UserValue::Str("Bob".to_string()));
        data1.insert("age".to_string(), UserValue::Int(25));
        let value1 = UserValue::Map(data1);

        let created = table.set(id, &value1).await.unwrap();
        assert!(created);
        assert_eq!(table.count().await.unwrap(), 1);

        // Now update with set
        let mut data2 = new_map();
        data2.insert("name".to_string(), UserValue::Str("Robert".to_string()));
        data2.insert("age".to_string(), UserValue::Int(26));
        data2.insert("city".to_string(), UserValue::Str("NYC".to_string()));
        let value2 = UserValue::Map(data2);

        let created_again = table.set(id, &value2).await.unwrap();
        assert!(!created_again, "Should return false for update");

        // Count should still be 1 (not incremented)
        assert_eq!(table.count().await.unwrap(), 1);

        // Verify updated value
        let retrieved = table.get(id).await.unwrap();
        assert_eq!(retrieved, value2);
    }

    #[tokio::test]
    async fn test_record_counter_with_insert_and_delete() {
        let (table, _dir) = create_test_table().await.unwrap();

        // Initial count should be 0
        assert_eq!(table.count().await.unwrap(), 0);

        // Insert 5 records
        let mut ids = vec![];
        for i in 0..5 {
            let mut data = new_map();
            data.insert("id".to_string(), UserValue::Int(i));
            let id = table.insert(&UserValue::Map(data)).await.unwrap();
            ids.push(id);
        }

        assert_eq!(table.count().await.unwrap(), 5);

        // Delete 2 records
        table.delete(ids[0]).await.unwrap();
        table.delete(ids[1]).await.unwrap();

        assert_eq!(table.count().await.unwrap(), 3);

        // Delete 1 more
        table.delete(ids[2]).await.unwrap();

        assert_eq!(table.count().await.unwrap(), 2);

        // Insert 3 more
        for i in 0..3 {
            let mut data = new_map();
            data.insert("new_id".to_string(), UserValue::Int(i));
            table.insert(&UserValue::Map(data)).await.unwrap();
        }

        assert_eq!(table.count().await.unwrap(), 5);
    }

    #[tokio::test]
    async fn test_set_method_respects_counter() {
        let (table, _dir) = create_test_table().await.unwrap();

        assert_eq!(table.count().await.unwrap(), 0);

        let id1 = RecordId::new();
        let id2 = RecordId::new();

        let mut data = new_map();
        data.insert("value".to_string(), UserValue::Int(42));

        // Create first record with set
        let created1 = table.set(id1, &UserValue::Map(data.clone())).await.unwrap();
        assert!(created1);
        assert_eq!(table.count().await.unwrap(), 1);

        // Create second record with set
        let created2 = table.set(id2, &UserValue::Map(data.clone())).await.unwrap();
        assert!(created2);
        assert_eq!(table.count().await.unwrap(), 2);

        // Update first record with set (count should not change)
        let updated = table.set(id1, &UserValue::Map(data.clone())).await.unwrap();
        assert!(!updated);
        assert_eq!(table.count().await.unwrap(), 2);

        // Update second record with set (count should not change)
        let updated2 = table.set(id2, &UserValue::Map(data.clone())).await.unwrap();
        assert!(!updated2);
        assert_eq!(table.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_counter_persistence_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_counter_db");
        let table_name = "counter_test";

        // === First session ===
        let repo1 = Arc::new(SledRepo::new(path.clone()).unwrap());
        let table1 = Table::new(repo1.clone(), table_name.to_string()).await.unwrap();

        // Insert some records
        for i in 0..10 {
            let mut data = new_map();
            data.insert("num".to_string(), UserValue::Int(i));
            table1.insert(&UserValue::Map(data)).await.unwrap();
        }

        assert_eq!(table1.count().await.unwrap(), 10);

        // Close first session
        drop(table1);
        drop(repo1);

        // === Second session ===
        let repo2 = Arc::new(SledRepo::new(path).unwrap());
        let table2 = Table::new(repo2, table_name.to_string()).await.unwrap();

        // Count should persist
        assert_eq!(table2.count().await.unwrap(), 10);

        // Insert more records
        for i in 10..15 {
            let mut data = new_map();
            data.insert("num".to_string(), UserValue::Int(i));
            table2.insert(&UserValue::Map(data)).await.unwrap();
        }

        assert_eq!(table2.count().await.unwrap(), 15);

        // Use set to create/update records
        let id = RecordId::new();
        let mut data = new_map();
        data.insert("test".to_string(), UserValue::Str("value".to_string()));
        let created = table2.set(id, &UserValue::Map(data)).await.unwrap();
        assert!(created);
        assert_eq!(table2.count().await.unwrap(), 16);

        // Update with set (count shouldn't change)
        let mut data2 = new_map();
        data2.insert("test".to_string(), UserValue::Str("updated".to_string()));
        let updated = table2.set(id, &UserValue::Map(data2)).await.unwrap();
        assert!(!updated);
        assert_eq!(table2.count().await.unwrap(), 16);
    }

    #[tokio::test]
    async fn test_counter_matches_actual_record_count() {
        let (table, _dir) = create_test_table().await.unwrap();

        // Perform various operations
        let mut ids = vec![];

        // Insert 5 records
        for i in 0..5 {
            let mut data = new_map();
            data.insert("i".to_string(), UserValue::Int(i));
            let id = table.insert(&UserValue::Map(data)).await.unwrap();
            ids.push(id);
        }

        // Use set to create 3 more
        for i in 0..3 {
            let id = RecordId::new();
            let mut data = new_map();
            data.insert("set_id".to_string(), UserValue::Int(i));
            table.set(id, &UserValue::Map(data)).await.unwrap();
        }

        // Delete 2 records
        table.delete(ids[0]).await.unwrap();
        table.delete(ids[1]).await.unwrap();

        // Use set to update a record (count shouldn't change)
        table.set(ids[2], &UserValue::Map(new_map())).await.unwrap();

        // Verify counter matches actual count
        let counter = table.count().await.unwrap();
        let actual = table.list().await.unwrap().len();
        assert_eq!(counter, actual);
        assert_eq!(counter, 6); // 5 insert - 2 delete + 3 set = 6
    }

    #[tokio::test]
    async fn test_counter_with_concurrent_operations() {
        let (table, _dir) = create_test_table().await.unwrap();

        let num_threads = 10;
        let records_per_thread = 5;
        let mut handles = vec![];

        // Concurrent inserts
        for _ in 0..num_threads {
            let table_clone = table.clone();
            handles.push(tokio::spawn(async move {
                for i in 0..records_per_thread {
                    let mut data = new_map();
                    data.insert("thread".to_string(), UserValue::Int(i));
                    table_clone.insert(&UserValue::Map(data)).await.unwrap();
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Verify count matches
        let expected = (num_threads * records_per_thread) as usize;
        let actual = table.count().await.unwrap();
        assert_eq!(actual, expected);
    }
}
