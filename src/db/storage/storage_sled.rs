use super::types::{Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use async_trait::async_trait;
use bytes::Bytes;
use sled::{Db, Tree};
use std::path::Path;
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
    async fn insert(&self, value: Bytes) -> DbResult<RecordId> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<RecordId> {
            let id = RecordId::new();
            let key = id.as_bytes();

            tree.insert(key, &*value)
                .map_err(|e| DbError::Storage(format!("SledDB insert: {}", e)))?;

            // sled is transactional by default, but we might want to flush explicitly for durability
            tree.flush()
                .map_err(|e| DbError::Storage(format!("SledDB flush: {}", e)))?;

            Ok(id)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordId, value: Bytes) -> DbResult<bool> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let key_bytes = key.as_bytes();

            let existed = tree
                .get(key_bytes)
                .map_err(|e| DbError::Storage(format!("SledDB get: {}", e)))?
                .is_some();

            tree.insert(key_bytes, &*value)
                .map_err(|e| DbError::Storage(format!("SledDB insert: {}", e)))?;

            tree.flush()
                .map_err(|e| DbError::Storage(format!("SledDB flush: {}", e)))?;

            Ok(!existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordId) -> DbResult<Bytes> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<Bytes> {
            let key_bytes = key.as_bytes();
            let val = tree
                .get(key_bytes)
                .map_err(|e| DbError::Storage(format!("SledDB get: {}", e)))?
                .ok_or_else(|| DbError::NotFound(key.to_string()))?;

            Ok(Bytes::copy_from_slice(&val))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn remove(&self, key: RecordId) -> DbResult<bool> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let key_bytes = key.as_bytes();
            let existed = tree
                .remove(key_bytes)
                .map_err(|e| DbError::Storage(format!("SledDB remove: {}", e)))?
                .is_some();

            tree.flush()
                .map_err(|e| DbError::Storage(format!("SledDB flush: {}", e)))?;

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn iter(&self) -> DbResult<Vec<(RecordId, Bytes)>> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<Vec<(RecordId, Bytes)>> {
            let mut out = Vec::new();
            for item in tree.iter() {
                let (key, val) =
                    item.map_err(|e| DbError::Storage(format!("SledDB iter item: {}", e)))?;
                let record_id = RecordId(key.as_ref().try_into().map_err(|_| {
                    DbError::Internal("Failed to convert key to RecordId".to_string())
                })?);
                out.push((record_id, Bytes::copy_from_slice(&val)));
            }
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
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
        let id1 = store1.insert(value1.to_bytes()).await.unwrap();

        // Insert into table2
        let value2 = InnerValue::Str("table2_value".to_string());
        let id2 = store2.insert(value2.to_bytes()).await.unwrap();

        // Verify isolation - each table should have only 1 record
        assert_eq!(store1.iter().await.unwrap().len(), 1);
        assert_eq!(store2.iter().await.unwrap().len(), 1);

        // Verify correct values
        let retrieved_bytes1 = store1.get(id1).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes1).unwrap(), value1);

        let retrieved_bytes2 = store2.get(id2).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes2).unwrap(), value2);

        // Verify cross-table isolation (get should fail with NotFound)
        assert!(matches!(store2.get(id1).await, Err(DbError::NotFound(_))));
        assert!(matches!(store1.get(id2).await, Err(DbError::NotFound(_))));

        // Clean up
        repo.store_delete("isolated_table1").await.unwrap();
        repo.store_delete("isolated_table2").await.unwrap();
    }
}
