use super::types::{RecordKey, Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::common::{new_dash_map, new_dash_map_wc, TDashMap};
use crate::types::record_id::RecordId;
use async_trait::async_trait;
use async_stream::stream;
use bytes::Bytes;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;

// ============================================================================
// InMemoryRepo - manages in-memory stores
// ============================================================================

pub struct InMemoryRepo {
    stores: Arc<TDashMap<String, Arc<InMemoryStore>>>,
}

impl InMemoryRepo {
    pub fn new() -> Self {
        Self {
            stores: Arc::new(new_dash_map_wc(1024)),
        }
    }
}

impl Default for InMemoryRepo {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Repo for InMemoryRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let name = name.as_ref();

        // Use DashMap's entry API for lock-free read or insert
        let entry = self.stores.entry(name.to_string()).or_insert_with(|| {
            Arc::new(InMemoryStore::new())
        });

        Ok(entry.value().clone() as Arc<dyn Store>)
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let name = name.as_ref();
        Ok(self.stores.remove(name).is_some())
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let names: Vec<String> = self.stores.iter().map(|kv| kv.key().clone()).collect();
        Ok(names)
    }
}

// ============================================================================
// InMemoryStore - individual in-memory store
// ============================================================================

pub struct InMemoryStore {
    data: Arc<TDashMap<RecordKey, Bytes>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            data: Arc::new(new_dash_map()),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Store for InMemoryStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let id = RecordId::new();
        let key = RecordKey::copy_from_slice(id.as_bytes());

        // DashMap entry API - try_entry returns Option<Entry>
        // If key exists, returns None. If not, returns Some(entry)
        match self.data.try_entry(key.clone()) {
            Some(entry) => {
                use dashmap::mapref::entry::Entry;
                match entry {
                    Entry::Vacant(vacant) => {
                        vacant.insert(value);
                        Ok(key)
                    }
                    Entry::Occupied(_) => Err(DbError::KeyExists(format!("Key already exists: {:?}", key))),
                }
            }
            None => Err(DbError::KeyExists(format!("Key already exists: {:?}", key))),
        }
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        // DashMap insert returns None if new, Some(old_value) if updated
        let existed = self.data.insert(key.clone(), value).is_some();
        Ok(!existed)
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.data
            .get(&key)
            .map(|ref_| ref_.value().clone())
            .ok_or_else(|| DbError::NotFound(format!("record not found: {:?}", key)))
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        Ok(self.data.remove(&key).is_some())
    }

    async fn iter(&self) -> DbResult<Vec<(RecordKey, Bytes)>> {
        let items: Vec<(RecordKey, Bytes)> = self
            .data
            .iter()
            .map(|ref_| (ref_.key().clone(), ref_.value().clone()))
            .collect();
        Ok(items)
    }

    fn iter_stream(&self, batch_size: usize) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let data = self.data.clone();

        Box::pin(stream! {
            let mut all_keys: Vec<RecordKey> = data
                .iter()
                .map(|ref_| ref_.key().clone())
                .collect();

            // Sort for consistent ordering
            all_keys.sort();

            while !all_keys.is_empty() {
                let batch: Vec<_> = all_keys
                    .drain(..std::cmp::min(batch_size, all_keys.len()))
                    .collect();

                let mut result = Vec::new();
                for key in batch {
                    if let Some(ref_) = data.get(&key) {
                        result.push((key, ref_.value().clone()));
                    }
                }

                if result.is_empty() {
                    break;
                }

                yield Ok(result);
            }
        })
    }

    async fn scan_prefix(&self, prefix: Bytes) -> DbResult<Vec<(RecordKey, Bytes)>> {
        let prefix_slice = &prefix[..];

        let items: Vec<(RecordKey, Bytes)> = self
            .data
            .iter()
            .filter(|ref_| ref_.key().starts_with(prefix_slice))
            .map(|ref_| (ref_.key().clone(), ref_.value().clone()))
            .collect();

        Ok(items)
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let data = self.data.clone();

        Box::pin(stream! {
            let prefix_slice = prefix.to_vec();
            let matching_keys: Vec<RecordKey> = data
                .iter()
                .filter(|ref_| ref_.key().starts_with(&prefix_slice[..]))
                .map(|ref_| ref_.key().clone())
                .collect();

            let mut keys = matching_keys;
            keys.sort();

            while !keys.is_empty() {
                let batch: Vec<_> = keys
                    .drain(..std::cmp::min(batch_size, keys.len()))
                    .collect();

                let mut result = Vec::new();
                for key in batch {
                    if let Some(ref_) = data.get(&key) {
                        result.push((key, ref_.value().clone()));
                    }
                }

                if result.is_empty() {
                    break;
                }

                yield Ok(result);
            }
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::value::InnerValue;
    use futures::StreamExt;
    use tokio::time::{sleep, Duration};

    async fn run_store_tests(store: Arc<dyn Store>) {
        // Test insert and get
        let value1 = InnerValue::Str("hello".to_string());
        let key1 = store.insert(value1.to_bytes()).await.unwrap();
        let retrieved_bytes = store.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes).unwrap(), value1);

        // Test set (update)
        sleep(Duration::from_micros(50)).await;
        let value2 = InnerValue::Str("world".to_string());
        let created = store.set(key1.clone(), value2.to_bytes()).await.unwrap();
        assert!(!created); // Should be false, as it's an update
        let retrieved_bytes2 = store.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes2).unwrap(), value2);

        // Test set (create)
        let id2 = RecordId::new();
        let key2 = Bytes::copy_from_slice(id2.as_bytes());
        let value3 = InnerValue::Int(123);
        let created2 = store.set(key2.clone(), value3.to_bytes()).await.unwrap();
        assert!(created2); // Should be true, as it's a new record
        let retrieved_bytes3 = store.get(key2.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes3).unwrap(), value3);

        // Test iter
        let value4 = InnerValue::Bool(true);
        let _key3 = store.insert(value4.to_bytes()).await.unwrap();
        let all_records = store.iter().await.unwrap();
        assert_eq!(all_records.len(), 3);
        assert!(all_records.iter().any(|(k, _)| *k == key1));
        assert!(all_records.iter().any(|(_, bytes)| InnerValue::from_bytes(bytes.clone()).unwrap() == value4));

        // Test remove
        assert!(store.remove(key1.clone()).await.unwrap());
        assert!(store.get(key1.clone()).await.is_err());
        assert!(!store.remove(key1).await.unwrap()); // Already removed

        let all_records_after_remove = store.iter().await.unwrap();
        assert_eq!(all_records_after_remove.len(), 2);
    }

    #[tokio::test]
    async fn test_inmemory_repo_basic() {
        let repo = InMemoryRepo::new();
        let store = repo.store_get("test_table").await.unwrap();

        run_store_tests(store).await;

        assert!(repo.store_delete("test_table").await.unwrap());
        assert!(!repo.store_delete("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_inmemory_repo_list_and_delete_stores() {
        let repo = InMemoryRepo::new();

        let _ = repo.store_get("table1").await.unwrap();
        let _ = repo.store_get("table2").await.unwrap();
        let _ = repo.store_get("table3").await.unwrap();

        let mut tables = repo.stores_list().await.unwrap();
        tables.sort(); // Order is not guaranteed
        assert_eq!(tables, vec!["table1", "table2", "table3"]);

        assert!(repo.store_delete("table2").await.unwrap());

        let mut tables_after_delete = repo.stores_list().await.unwrap();
        tables_after_delete.sort();
        assert_eq!(tables_after_delete, vec!["table1", "table3"]);
    }

    #[tokio::test]
    async fn test_inmemory_prefix_scan() {
        let store = InMemoryStore::new();

        // Insert records with composite keys
        let data = vec![
            (b"country:Russia:Moscow:user1".to_vec(), InnerValue::Str("Alice".to_string())),
            (b"country:Russia:Moscow:user2".to_vec(), InnerValue::Str("Bob".to_string())),
            (b"country:Russia:SPb:user3".to_vec(), InnerValue::Str("Charlie".to_string())),
            (b"country:France:Paris:user4".to_vec(), InnerValue::Str("David".to_string())),
            (b"country:France:Lyon:user5".to_vec(), InnerValue::Str("Eve".to_string())),
        ];

        for (key, value) in &data {
            store.set(key.clone().into(), value.to_bytes()).await.unwrap();
        }

        // Test prefix scan for "country:Russia:Moscow:"
        let results = store
            .scan_prefix(b"country:Russia:Moscow:".to_vec().into())
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|(k, _)| k.as_ref() == b"country:Russia:Moscow:user1"));
        assert!(results.iter().any(|(k, _)| k.as_ref() == b"country:Russia:Moscow:user2"));

        // Test prefix scan for "country:Russia:"
        let results_russia = store.scan_prefix(b"country:Russia:".to_vec().into()).await.unwrap();
        assert_eq!(results_russia.len(), 3);

        // Test prefix scan for "country:France:"
        let results_france = store.scan_prefix(b"country:France:".to_vec().into()).await.unwrap();
        assert_eq!(results_france.len(), 2);

        // Test streaming prefix scan
        let mut stream = store.scan_prefix_stream(b"country:Russia:".to_vec().into(), 2);
        let mut all_records = Vec::new();
        let mut batch_count = 0;

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.unwrap();
            batch_count += 1;
            all_records.extend(batch);
        }

        assert_eq!(all_records.len(), 3);
        assert_eq!(batch_count, 2); // 2 + 1 = 3
    }

    #[tokio::test]
    async fn test_inmemory_iter_stream() {
        let store = InMemoryStore::new();

        // Insert 25 records
        let mut expected_keys = Vec::new();
        for i in 0..25 {
            let value = InnerValue::Int(i);
            let key = store.insert(value.to_bytes()).await.unwrap();
            expected_keys.push(key);
        }

        // Test streaming with batch_size=10
        let mut stream = store.iter_stream(10);
        let mut all_records = Vec::new();
        let mut batch_count = 0;

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.unwrap();
            batch_count += 1;
            all_records.extend(batch);
        }

        assert_eq!(all_records.len(), 25);
        assert_eq!(batch_count, 3); // 10 + 10 + 5 = 25

        // Verify all keys are present
        for key in &expected_keys {
            assert!(all_records.iter().any(|(rec_key, _)| rec_key == key));
        }
    }

    #[tokio::test]
    async fn test_inmemory_concurrent_access() {
        use tokio::task::JoinSet;

        let store = Arc::new(InMemoryStore::new());
        let mut join_set = JoinSet::new();

        // Spawn 100 concurrent writes
        for i in 0..100 {
            let store_clone = store.clone();
            join_set.spawn(async move {
                let key = format!("key_{}", i);
                let value = Bytes::from(key.clone());
                store_clone.set(key.into(), value).await.unwrap();
            });
        }

        // Spawn 100 concurrent reads while writes are happening
        for i in 0..100 {
            let store_clone = store.clone();
            join_set.spawn(async move {
                let key = format!("key_{}", i);
                let _ = store_clone.get(key.into()).await;
            });
        }

        // All tasks should complete without deadlocking
        while let Some(result) = join_set.join_next().await {
            result.unwrap();
        }

        // Verify all writes succeeded
        let all_records = store.iter().await.unwrap();
        assert_eq!(all_records.len(), 100);
    }
}
