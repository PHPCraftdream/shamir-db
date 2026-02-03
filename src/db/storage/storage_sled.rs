use super::types::{RecordKey, Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use async_trait::async_trait;
use async_stream::stream;
use bytes::Bytes;
use futures::stream::Stream;
use sled::{Db, Tree};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task::spawn_blocking;

// ============================================================================
// SledRepo - manages multiple stores (trees)
// ============================================================================

#[derive(Clone)]
pub struct SledRepo {
    db: Arc<Db>,
}

impl SledRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let db = sled::open(path).map_err(|e| DbError::Storage(format!("SledDB open: {}", e)))?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl Repo for SledRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();

        let tree = spawn_blocking(move || -> DbResult<Tree> {
            db.open_tree(table_name.as_bytes())
                .map_err(|e| DbError::Storage(format!("SledDB open_tree: {}", e)))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))??;

        let store = SledStore {
            tree: Arc::new(tree),
        };
        Ok(Arc::new(store))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();

        spawn_blocking(move || -> DbResult<bool> {
            db.drop_tree(table_name.as_bytes())
                .map_err(|e| DbError::Storage(format!("SledDB drop_tree: {}", e)))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let db = self.db.clone();
        spawn_blocking(move || -> DbResult<Vec<String>> {
            let names = db
                .tree_names()
                .into_iter()
                .filter(|name| *name != b"__sled__default")
                .map(|bytes| String::from_utf8(bytes.to_vec()))
                .collect::<Result<Vec<String>, _>>()
                .map_err(|e| DbError::Codec(format!("UTF-8 decode error: {}", e)))?;
            Ok(names)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }
}

// ============================================================================
// SledStore - individual store (tree)
// ============================================================================

pub struct SledStore {
    tree: Arc<Tree>,
}

unsafe impl Send for SledStore {}
unsafe impl Sync for SledStore {}

#[async_trait]
impl Store for SledStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<RecordKey> {
            let id = RecordId::new();
            let key = RecordKey::copy_from_slice(id.as_bytes());

            tree.insert(&key[..], &*value)
                .map_err(|e| DbError::Storage(format!("SledDB insert: {}", e)))?;

            // sled is transactional by default, but we might want to flush explicitly for durability
            tree.flush()
                .map_err(|e| DbError::Storage(format!("SledDB flush: {}", e)))?;

            Ok(key)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let existed = tree
                .get(&key[..])
                .map_err(|e| DbError::Storage(format!("SledDB get: {}", e)))?
                .is_some();

            tree.insert(&key[..], &*value)
                .map_err(|e| DbError::Storage(format!("SledDB insert: {}", e)))?;

            tree.flush()
                .map_err(|e| DbError::Storage(format!("SledDB flush: {}", e)))?;

            Ok(!existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<Bytes> {
            let val = tree
                .get(&key[..])
                .map_err(|e| DbError::Storage(format!("SledDB get: {}", e)))?
                .ok_or_else(|| DbError::NotFound(format!("{:?}", key)))?;

            Ok(Bytes::copy_from_slice(&val))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let existed = tree
                .remove(&key[..])
                .map_err(|e| DbError::Storage(format!("SledDB remove: {}", e)))?
                .is_some();

            tree.flush()
                .map_err(|e| DbError::Storage(format!("SledDB flush: {}", e)))?;

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn iter(&self) -> DbResult<Vec<(RecordKey, Bytes)>> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<Vec<(RecordKey, Bytes)>> {
            let mut out = Vec::new();
            for item in tree.iter() {
                let (key, val) =
                    item.map_err(|e| DbError::Storage(format!("SledDB iter item: {}", e)))?;
                out.push((Bytes::copy_from_slice(&key), Bytes::copy_from_slice(&val)));
            }
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    fn iter_stream(&self, batch_size: usize) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let tree = self.tree.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            loop {
                // Fetch next batch in spawn_blocking
                let tree_clone = tree.clone();
                let start_key = last_key.clone();

                let batch: DbResult<Vec<_>> = spawn_blocking(move || {
                    let iter = if let Some(ref start) = start_key {
                        tree_clone.range::<&[u8], _>(start.as_slice()..)
                    } else {
                        tree_clone.iter()
                    };

                    let mut items = Vec::new();
                    let mut skip_first = start_key.is_some();

                    for item in iter {
                        if skip_first {
                            skip_first = false;
                            continue; // Skip the cursor record itself
                        }

                        if items.len() >= batch_size {
                            break;
                        }

                        let (key, val) = item.map_err(|e| DbError::Storage(format!("SledDB iter error: {}", e)))?;
                        items.push((Bytes::copy_from_slice(&key), Bytes::copy_from_slice(&val)));
                    }
                    Ok(items)
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let batch = batch?;

                if batch.is_empty() {
                    break; // No more records
                }

                // Remember last key for next iteration
                last_key = batch.last().map(|(k, _)| k.to_vec());

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
    use crate::types::record_id::RecordId;
    use crate::types::value::InnerValue;
    use futures::StreamExt;
    use std::fs;
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
    async fn test_sled_repo_basic() {
        let path = "./test_data/sled_repo_basic";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = SledRepo::new(path).unwrap();
        let store = repo.store_get("test_table").await.unwrap();

        run_store_tests(store).await;

        assert!(repo.store_delete("test_table").await.unwrap());
    }

    #[tokio::test]
    async fn test_sled_repo_list_stores() {
        let path = "./test_data/sled_repo_list";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = SledRepo::new(path).unwrap();

        // Create first store
        let _store1 = repo.store_get("table1").await.unwrap();
        let tables = repo.stores_list().await.unwrap();
        assert_eq!(tables.len(), 1);
        assert!(tables.contains(&"table1".to_string()));

        // Create second store
        let _store2 = repo.store_get("table2").await.unwrap();
        let tables = repo.stores_list().await.unwrap();
        assert_eq!(tables.len(), 2);
        assert!(tables.contains(&"table1".to_string()));
        assert!(tables.contains(&"table2".to_string()));

        // Delete one store
        assert!(repo.store_delete("table1").await.unwrap());
        let tables = repo.stores_list().await.unwrap();
        assert_eq!(tables.len(), 1);
        assert!(!tables.contains(&"table1".to_string()));
        assert!(tables.contains(&"table2".to_string()));
    }

    #[tokio::test]
    async fn test_sled_repo_store_isolation() {
        let path = "./test_data/sled_repo_isolation";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = SledRepo::new(path).unwrap();

        let store1 = repo.store_get("isolated_table1").await.unwrap();
        let store2 = repo.store_get("isolated_table2").await.unwrap();

        // Insert into table1
        let value1 = InnerValue::Str("table1_value".to_string());
        let key1 = store1.insert(value1.to_bytes()).await.unwrap();

        // Insert into table2
        let value2 = InnerValue::Str("table2_value".to_string());
        let key2 = store2.insert(value2.to_bytes()).await.unwrap();

        // Verify isolation - each table should have only 1 record
        assert_eq!(store1.iter().await.unwrap().len(), 1);
        assert_eq!(store2.iter().await.unwrap().len(), 1);

        // Verify correct values
        let retrieved_bytes1 = store1.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes1).unwrap(), value1);

        let retrieved_bytes2 = store2.get(key2.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes2).unwrap(), value2);

        // Verify cross-table isolation (get should fail with NotFound)
        assert!(matches!(store2.get(key1).await, Err(DbError::NotFound(_))));
        assert!(matches!(store1.get(key2).await, Err(DbError::NotFound(_))));

        // Clean up
        repo.store_delete("isolated_table1").await.unwrap();
        repo.store_delete("isolated_table2").await.unwrap();
    }

    #[tokio::test]
    async fn test_sled_iter_stream() {
        let path = "./test_data/sled_iter_stream";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = SledRepo::new(path).unwrap();
        let store = repo.store_get("test_table").await.unwrap();

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
            println!("Batch {} has {} records", batch_count, batch.len());
            all_records.extend(batch);
        }

        assert_eq!(all_records.len(), 25);
        assert_eq!(batch_count, 3); // 10 + 10 + 5 = 25

        // Verify all keys are present
        for key in &expected_keys {
            assert!(all_records.iter().any(|(rec_key, _)| rec_key == key));
        }
    }
}
