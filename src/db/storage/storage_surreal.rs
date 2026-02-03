use super::types::{Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use async_trait::async_trait;
use bytes::Bytes;
use surrealkv::{Tree, TreeBuilder};
use std::path::Path;
use std::sync::Arc;

// ============================================================================
// SurrealKVRepo - manages multiple stores (tables)
// ============================================================================

pub struct SurrealKVRepo {
    tree: Arc<Tree>,
}

impl SurrealKVRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let path = path.as_ref();

        // FIX for Windows: Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| DbError::Storage(format!("Failed to create directory: {}", e)))?;
            }
        }

        let tree = TreeBuilder::new()
            .with_path(path.to_path_buf())
            .build()
            .map_err(|e| DbError::Storage(format!("SurrealKV build: {}", e)))?;

        Ok(Self {
            tree: Arc::new(tree),
        })
    }

    async fn register_store(&self, table_name: &str) -> DbResult<()> {
        let mut txn = self.tree.begin().map_err(|e| DbError::Storage(format!("SurrealKV begin: {}", e)))?;
        let tables_key = b"__system__:__tables__";

        let mut tables = txn
            .get(tables_key)
            .map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))?
            .map(|v| decode_tables(&v))
            .unwrap_or_default();

        if !tables.contains(&table_name.to_string()) {
            tables.push(table_name.to_string());
            txn.set(tables_key.to_vec(), encode_tables(&tables))
                .map_err(|e| DbError::Storage(format!("SurrealKV set: {}", e)))?;
            txn.commit()
                .await
                .map_err(|e| DbError::Storage(format!("SurrealKV commit: {}", e)))?;
        }

        Ok(())
    }

    async fn unregister_store(&self, table_name: &str) -> DbResult<bool> {
        let mut txn = self.tree.begin().map_err(|e| DbError::Storage(format!("SurrealKV begin: {}", e)))?;
        let tables_key = b"__system__:__tables__";

        let existed = if let Some(v) = txn.get(tables_key).map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))? {
            let mut tables = decode_tables(&v);
            let existed = tables.contains(&table_name.to_string());
            tables.retain(|t| t != table_name);
            txn.set(tables_key.to_vec(), encode_tables(&tables))
                .map_err(|e| DbError::Storage(format!("SurrealKV set: {}", e)))?;
            txn.commit()
                .await
                .map_err(|e| DbError::Storage(format!("SurrealKV commit: {}", e)))?;
            existed
        } else {
            false
        };

        Ok(existed)
    }

    pub async fn store_get_by_name(&self, name: &str) -> DbResult<Arc<dyn Store>> {
        self.register_store(name).await?;

        Ok(Arc::new(SurrealKVStore {
            tree: self.tree.clone(),
            table_name: name.to_string(),
        }))
    }

    pub async fn store_delete_by_name(&self, name: &str) -> DbResult<bool> {
        let mut txn = self.tree.begin().map_err(|e| DbError::Storage(format!("SurrealKV begin: {}", e)))?;
        let idx_key = format!("__table__:{}:__keys__", name).into_bytes();

        let ids = txn
            .get(&idx_key)
            .map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();

        if ids.is_empty() {
            drop(txn);
            // Return false - no data to delete
            return Ok(false);
        }

        // Delete all records
        for id in &ids {
            txn.delete(prefixed_key(name, *id))
                .map_err(|e| DbError::Storage(format!("SurrealKV delete: {}", e)))?;
        }

        // Delete index
        txn.delete(idx_key)
            .map_err(|e| DbError::Storage(format!("SurrealKV delete: {}", e)))?;
        txn.commit()
            .await
            .map_err(|e| DbError::Storage(format!("SurrealKV commit: {}", e)))?;

        Ok(true)
    }
}

#[async_trait]
impl Repo for SurrealKVRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        self.store_get_by_name(name.as_ref()).await
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        // First delete the data
        let data_deleted = self.store_delete_by_name(name.as_ref()).await?;

        // Then unregister from tables list
        let table_existed = self.unregister_store(name.as_ref()).await?;

        Ok(data_deleted || table_existed)
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let txn = self.tree.begin().map_err(|e| DbError::Storage(format!("SurrealKV begin: {}", e)))?;
        let tables_key = b"__system__:__tables__";

        let tables = txn
            .get(tables_key)
            .map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))?
            .map(|v| decode_tables(&v))
            .unwrap_or_default();

        Ok(tables)
    }
}

// ============================================================================
// SurrealKVStore - individual store (table)
// ============================================================================

pub struct SurrealKVStore {
    tree: Arc<Tree>,
    table_name: String,
}

unsafe impl Send for SurrealKVStore {}
unsafe impl Sync for SurrealKVStore {}

impl SurrealKVStore {
    // Helper methods are inline in the impl Store above
}

#[async_trait]
impl Store for SurrealKVStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordId> {
        let mut txn = self.tree.begin().map_err(|e| DbError::Storage(format!("SurrealKV begin: {}", e)))?;

        let id = RecordId::new();
        let key = prefixed_key(&self.table_name, id);

        // Check if key exists
        if txn.get(&key).map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))?.is_some() {
            return Err(DbError::KeyExists(format!("Key exists: {:?}", id)));
        }

        txn.set(key, value.to_vec())
            .map_err(|e| DbError::Storage(format!("SurrealKV set: {}", e)))?;

        let idx_key = index_key_for_table(&self.table_name);
        let mut ids = txn
            .get(&idx_key)
            .map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();
        ids.push(id);
        txn.set(idx_key, encode_ids(&ids))
            .map_err(|e| DbError::Storage(format!("SurrealKV set: {}", e)))?;

        txn.commit()
            .await
            .map_err(|e| DbError::Storage(format!("SurrealKV commit: {}", e)))?;
        Ok(id)
    }

    async fn set(&self, key: RecordId, value: Bytes) -> DbResult<bool> {
        let mut txn = self.tree.begin().map_err(|e| DbError::Storage(format!("SurrealKV begin: {}", e)))?;
        let key_bytes = prefixed_key(&self.table_name, key);

        let existed = txn
            .get(&key_bytes)
            .map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))?
            .is_some();

        txn.set(key_bytes, value.to_vec())
            .map_err(|e| DbError::Storage(format!("SurrealKV set: {}", e)))?;

        let idx_key = index_key_for_table(&self.table_name);
        let mut ids = txn
            .get(&idx_key)
            .map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();

        if !ids.contains(&key) {
            ids.push(key);
            txn.set(idx_key, encode_ids(&ids))
                .map_err(|e| DbError::Storage(format!("SurrealKV set: {}", e)))?;
        }

        txn.commit()
            .await
            .map_err(|e| DbError::Storage(format!("SurrealKV commit: {}", e)))?;

        Ok(!existed)
    }

    async fn get(&self, key: RecordId) -> DbResult<Bytes> {
        let txn = self.tree.begin().map_err(|e| DbError::Storage(format!("SurrealKV begin: {}", e)))?;
        let key_bytes = prefixed_key(&self.table_name, key);

        let val = txn
            .get(&key_bytes)
            .map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))?
            .ok_or_else(|| DbError::NotFound(key.to_string()))?;

        Ok(Bytes::copy_from_slice(&val))
    }

    async fn remove(&self, key: RecordId) -> DbResult<bool> {
        let mut txn = self.tree.begin().map_err(|e| DbError::Storage(format!("SurrealKV begin: {}", e)))?;
        let key_bytes = prefixed_key(&self.table_name, key);

        let existed = txn
            .get(&key_bytes)
            .map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))?
            .is_some();

        if existed {
            txn.delete(key_bytes)
                .map_err(|e| DbError::Storage(format!("SurrealKV delete: {}", e)))?;

            let idx_key = index_key_for_table(&self.table_name);
            if let Some(v) = txn.get(&idx_key).map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))? {
                let mut ids = decode_ids(&v);
                ids.retain(|&k| k != key);
                txn.set(idx_key, encode_ids(&ids))
                    .map_err(|e| DbError::Storage(format!("SurrealKV set: {}", e)))?;
            }

            txn.commit()
                .await
                .map_err(|e| DbError::Storage(format!("SurrealKV commit: {}", e)))?;
        }

        Ok(existed)
    }

    async fn iter(&self) -> DbResult<Vec<(RecordId, Bytes)>> {
        let txn = self.tree.begin().map_err(|e| DbError::Storage(format!("SurrealKV begin: {}", e)))?;
        let idx_key = index_key_for_table(&self.table_name);

        let ids = txn
            .get(&idx_key)
            .map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();

        let mut out = Vec::new();
        for id in ids {
            let key = prefixed_key(&self.table_name, id);
            if let Some(val) = txn.get(&key).map_err(|e| DbError::Storage(format!("SurrealKV get: {}", e)))? {
                out.push((id, Bytes::copy_from_slice(&val)));
            }
        }
        Ok(out)
    }
}

// ============================================================================
// Helper functions
// ============================================================================

fn prefixed_key(table_name: &str, key: RecordId) -> Vec<u8> {
    let mut k = table_name.as_bytes().to_vec();
    k.push(b':');
    k.extend_from_slice(key.as_bytes());
    k
}

fn index_key_for_table(table_name: &str) -> Vec<u8> {
    format!("__table__:{}:__keys__", table_name).into_bytes()
}

fn decode_ids(data: &[u8]) -> Vec<RecordId> {
    rmp_serde::from_slice(data).unwrap_or_default()
}

fn encode_ids(ids: &[RecordId]) -> Vec<u8> {
    rmp_serde::to_vec(ids).unwrap_or_default()
}

fn decode_tables(data: &[u8]) -> Vec<String> {
    rmp_serde::from_slice(data).unwrap_or_default()
}

fn encode_tables(tables: &[String]) -> Vec<u8> {
    rmp_serde::to_vec(tables).unwrap_or_default()
}

// ============================================================================
// Tests
// ============================================================================
#[cfg(test)]
#[cfg(not(target_os = "windows"))]  // SurrealKV has known issues on Windows (Access denied errors)
mod tests {
    use super::*;
    use crate::types::value::InnerValue;
    use std::fs;
    use tokio::time::{sleep, Duration};
    use serial_test::serial;

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
    #[serial]
    async fn test_surrealkv_repo_basic() {
        let path = "./test_data/surrealkv_basic";

        {
            let repo = SurrealKVRepo::new(path).unwrap();
            let store = repo.store_get("test_table").await.unwrap();

            run_store_tests(store.clone()).await;

            assert!(repo.store_delete("test_table").await.unwrap());

            drop(store);
            drop(repo);
        }

        // Wait for file handles to be released
        sleep(Duration::from_millis(200)).await;

        // Clean up after test
        let _ = fs::remove_dir_all(path);
    }

    #[tokio::test]
    #[serial]
    async fn test_surrealkv_repo_list_stores() {
        let path = "./test_data/surrealkv_list";

        {
            let repo = SurrealKVRepo::new(path).unwrap();

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

            // Create third store
            let _store3 = repo.store_get("table3").await.unwrap();
            let tables = repo.stores_list().await.unwrap();
            assert_eq!(tables.len(), 3);
            assert!(tables.contains(&"table1".to_string()));
            assert!(tables.contains(&"table2".to_string()));
            assert!(tables.contains(&"table3".to_string()));

            // Delete one store
            assert!(repo.store_delete("table2").await.unwrap());
            let tables = repo.stores_list().await.unwrap();
            assert_eq!(tables.len(), 2);
            assert!(tables.contains(&"table1".to_string()));
            assert!(!tables.contains(&"table2".to_string()));
            assert!(tables.contains(&"table3".to_string()));

            // Delete all remaining stores
            assert!(repo.store_delete("table1").await.unwrap());
            assert!(repo.store_delete("table3").await.unwrap());

            // Verify all stores deleted
            let _store_check = repo.store_get("check_table").await.unwrap();
            let tables = repo.stores_list().await.unwrap();
            assert_eq!(tables.len(), 1);
            assert!(tables.contains(&"check_table".to_string()));

            drop(repo);
        }

        sleep(Duration::from_millis(200)).await;
        let _ = fs::remove_dir_all(path);
    }

    #[tokio::test]
    #[serial]
    async fn test_surrealkv_repo_store_isolation() {
        let path = "./test_data/surrealkv_isolation";

        {
            let repo = SurrealKVRepo::new(path).unwrap();

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

            // Verify cross-table isolation
            assert!(store2.get(id1).await.is_err());
            assert!(store1.get(id2).await.is_err());

            // Clean up
            repo.store_delete("isolated_table1").await.unwrap();
            repo.store_delete("isolated_table2").await.unwrap();

            drop(store1);
            drop(store2);
            drop(repo);
        }

        sleep(Duration::from_millis(200)).await;
        let _ = fs::remove_dir_all(path);
    }
}