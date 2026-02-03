use super::types::{Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use async_trait::async_trait;
use async_stream::stream;
use bytes::Bytes;
use fjall::{Database, Keyspace, KeyspaceCreateOptions};
use futures::stream::Stream;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task;

// ============================================================================
// FjallRepo - manages database connection
// ============================================================================

pub struct FjallRepo {
    db: Arc<Database>,
}

impl FjallRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let db = Database::builder(path.as_ref())
            .open()
            .map_err(|e| DbError::Storage(e.to_string()))?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl Repo for FjallRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let keyspace = self
            .db
            .keyspace(name.as_ref(), || KeyspaceCreateOptions::default())
            .map_err(|e| DbError::Storage(e.to_string()))?;
        Ok(Arc::new(FjallStore { keyspace }))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        // Get the keyspace first
        let keyspace = self
            .db
            .keyspace(name.as_ref(), || KeyspaceCreateOptions::default())
            .map_err(|e| DbError::Storage(e.to_string()))?;

        // Then delete it using the keyspace handle
        self.db
            .delete_keyspace(keyspace)
            .map_err(|e| DbError::Storage(e.to_string()))?;
        Ok(true)
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let names: Vec<String> = self
            .db
            .list_keyspace_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        Ok(names)
    }
}

// ============================================================================
// FjallStore - individual store (keyspace)
// ============================================================================

pub struct FjallStore {
    keyspace: Keyspace,
}

#[async_trait]
impl Store for FjallStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordId> {
        let id = RecordId::new();
        let keyspace = self.keyspace.clone();

        task::spawn_blocking(move || -> DbResult<RecordId> {
            // Check if key exists first
            if keyspace
                .contains_key(id.as_bytes())
                .map_err(|e| DbError::Storage(e.to_string()))?
            {
                return Err(DbError::KeyExists(format!("Key already exists: {:?}", id)));
            }

            // Insert the value
            keyspace
                .insert(id.as_bytes(), &*value)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            Ok(id)
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn set(&self, key: RecordId, value: Bytes) -> DbResult<bool> {
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<bool> {
            // Check if key existed before
            let existed = keyspace
                .contains_key(key.as_bytes())
                .map_err(|e| DbError::Storage(e.to_string()))?;

            // Insert/update the value
            keyspace
                .insert(key.as_bytes(), &*value)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            // Return true if created (didn't exist), false if updated (existed)
            Ok(!existed)
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn get(&self, key: RecordId) -> DbResult<Bytes> {
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<Bytes> {
            match keyspace.get(key.as_bytes()).map_err(|e| DbError::Storage(e.to_string()))?
            {
                Some(slice) => Ok(Bytes::copy_from_slice(&slice)),
                None => Err(DbError::NotFound(format!("record not found: {:}", key))),
            }
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn remove(&self, key: RecordId) -> DbResult<bool> {
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<bool> {
            // Check if key exists
            let existed = keyspace
                .contains_key(key.as_bytes())
                .map_err(|e| DbError::Storage(e.to_string()))?;

            if existed {
                keyspace
                    .remove(key.as_bytes())
                    .map_err(|e| DbError::Storage(e.to_string()))?;
            }

            Ok(existed)
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn iter(&self) -> DbResult<Vec<(RecordId, Bytes)>> {
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<Vec<(RecordId, Bytes)>> {
            let mut items = Vec::new();

            // Iterate over all items - Guard::into_inner() returns key-value pair directly
            for guard in keyspace.iter() {
                let (key, value_slice) = guard
                    .into_inner()
                    .map_err(|e| DbError::Storage(e.to_string()))?;

                let record_id = RecordId(key.as_ref().try_into().map_err(|_| {
                    DbError::Internal("Failed to convert key to RecordId".to_string())
                })?);

                items.push((record_id, Bytes::copy_from_slice(&value_slice)));
            }

            Ok(items)
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))?
    }

    fn iter_stream(&self, batch_size: usize) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordId, Bytes)>, DbError>> + Send>> {
        let keyspace = self.keyspace.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            loop {
                let keyspace_clone = keyspace.clone();
                let start_key = last_key;

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = task::spawn_blocking(move || {
                    let mut items = Vec::new();
                    let mut last_batch_key: Option<Vec<u8>> = None;

                    let mut iter = keyspace_clone.iter();

                    // If cursor specified, skip until we pass it
                    if let Some(start) = start_key {
                        while let Some(guard) = iter.next() {
                            let (key, _) = guard
                                .into_inner()
                                .map_err(|e| DbError::Storage(e.to_string()))?;
                            if key.as_ref() == start.as_slice() {
                                // Found it, next item will be first in batch
                                break;
                            }
                        }
                    }

                    for guard in iter.take(batch_size) {
                        let (key, value_slice) = guard
                            .into_inner()
                            .map_err(|e| DbError::Storage(e.to_string()))?;

                        last_batch_key = Some(key.to_vec());
                        let record_id = RecordId(key.as_ref().try_into().map_err(|_| {
                            DbError::Internal("Failed to convert key to RecordId".to_string())
                        })?);
                        items.push((record_id, Bytes::copy_from_slice(&value_slice)));
                    }

                    Ok((items, last_batch_key))
                })
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

                let (batch, next_key) = batch?;

                if batch.is_empty() {
                    break;
                }

                last_key = next_key;
                yield Ok(batch);
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
    use std::fs;
    use tokio::time::{sleep, Duration};

    async fn run_store_tests(store: Arc<dyn Store>) {
        // Test insert and get
        let value1 = InnerValue::Str("hello".to_string());
        let id1 = store.insert(value1.to_bytes()).await.unwrap();
        let retrieved_bytes = store.get(id1).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes).unwrap(), value1);

        // Test set (update)
        sleep(Duration::from_micros(50)).await;
        let value2 = InnerValue::Str("world".to_string());
        let created = store.set(id1, value2.to_bytes()).await.unwrap();
        assert!(!created); // Should be false, as it's an update
        let retrieved_bytes2 = store.get(id1).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes2).unwrap(), value2);

        // Test set (create)
        let id2 = RecordId::new();
        let value3 = InnerValue::Int(123);
        let created2 = store.set(id2, value3.to_bytes()).await.unwrap();
        assert!(created2); // Should be true, as it's a new record
        let retrieved_bytes3 = store.get(id2).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes3).unwrap(), value3);

        // Test iter
        let value4 = InnerValue::Bool(true);
        let _id3 = store.insert(value4.to_bytes()).await.unwrap();
        let all_records = store.iter().await.unwrap();
        assert_eq!(all_records.len(), 3);
        assert!(all_records.iter().any(|(id, _)| *id == id1));
        assert!(all_records.iter().any(|(_, bytes)| InnerValue::from_bytes(bytes.clone()).unwrap() == value4));

        // Test remove
        assert!(store.remove(id1).await.unwrap());
        assert!(store.get(id1).await.is_err());
        assert!(!store.remove(id1).await.unwrap()); // Already removed

        let all_records_after_remove = store.iter().await.unwrap();
        assert_eq!(all_records_after_remove.len(), 2);
    }

    #[tokio::test]
    async fn test_fjall_repo_basic() {
        let path = "./test_data/fjall_repo_basic";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = FjallRepo::new(path).unwrap();
        let store = repo.store_get("test_table").await.unwrap();

        run_store_tests(store).await;

        assert!(repo.store_delete("test_table").await.unwrap());
    }

    #[tokio::test]
    async fn test_fjall_repo_list_and_delete_stores() {
        let path = "./test_data/fjall_repo_list";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = FjallRepo::new(path).unwrap();

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
}