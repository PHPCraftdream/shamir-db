use super::types::{Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use crate::types::repo_record::RepoRecord;
use crate::types::value::InnerValue;
use async_trait::async_trait;
use chrono::Utc;
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
        let tree = TreeBuilder::new()
            .with_path(path.as_ref().to_path_buf())
            .build()
            .map_err(|e| DbError::Storage(e.to_string()))?;

        Ok(Self {
            tree: Arc::new(tree),
        })
    }

    async fn register_store(&self, table_name: &str) -> DbResult<()> {
        let mut txn = self.tree.begin()?;
        let tables_key = b"__system__:__tables__";

        let mut tables = txn
            .get(tables_key)?
            .map(|v| decode_tables(&v))
            .unwrap_or_default();

        if !tables.contains(&table_name.to_string()) {
            tables.push(table_name.to_string());
            txn.set(tables_key.to_vec(), encode_tables(&tables))?;
            txn.commit().await?;
        }

        Ok(())
    }

    async fn unregister_store(&self, table_name: &str) -> DbResult<bool> {
        let mut txn = self.tree.begin()?;
        let tables_key = b"__system__:__tables__";

        let existed = if let Some(v) = txn.get(tables_key)? {
            let mut tables = decode_tables(&v);
            let existed = tables.contains(&table_name.to_string());
            tables.retain(|t| t != table_name);
            txn.set(tables_key.to_vec(), encode_tables(&tables))?;
            txn.commit().await?;
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
        let mut txn = self.tree.begin()?;
        let idx_key = format!("__table__:{}:__keys__", name).into_bytes();

        let ids = txn
            .get(&idx_key)?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();

        if ids.is_empty() {
            // Even if no data, we might have the table registered
            drop(txn); // Release transaction before calling unregister_store
            return self.unregister_store(name).await;
        }

        // Delete all records
        for id in &ids {
            txn.delete(prefixed_key(name, *id))?;
        }

        // Delete index
        txn.delete(idx_key)?;
        txn.commit().await?;

        // Remove from global tables list
        self.unregister_store(name).await  // <- добавлен .await
    }
}

#[async_trait]
impl Repo for SurrealKVRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        self.store_get_by_name(name.as_ref()).await
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        self.store_delete_by_name(name.as_ref()).await
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let txn = self.tree.begin()?;
        let tables_key = b"__system__:__tables__";

        let tables = txn
            .get(tables_key)?
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
    fn index_key(&self) -> Vec<u8> {
        format!("__table__:{}:__keys__", self.table_name).into_bytes()
    }
}

#[async_trait]
impl Store for SurrealKVStore {
    async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        let mut txn = self.tree.begin()?;

        let id = RecordId::new();
        let now = Utc::now().timestamp_micros() as u64;
        let record: RepoRecord = (id, now, now, value.clone());

        let key = prefixed_key(&self.table_name, id);
        if txn.get(&key)?.is_some() {
            return Err(DbError::Internal(format!("Key exists: {:?}", id)));
        }

        txn.set(key, rmp_serde::to_vec(&record)?)?;

        let idx_key = self.index_key();
        let mut ids = txn
            .get(&idx_key)?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();
        ids.push(id);
        txn.set(idx_key, encode_ids(&ids))?;

        txn.commit().await?;
        Ok(id)
    }

    async fn set(&self, key: RecordId, value: &InnerValue) -> DbResult<bool> {
        let mut txn = self.tree.begin()?;
        let key_bytes = prefixed_key(&self.table_name, key);

        let existing = txn
            .get(&key_bytes)?
            .and_then(|v| rmp_serde::from_slice::<RepoRecord>(&v).ok());

        let created_at = existing.map_or(Utc::now().timestamp_micros() as u64, |r| r.1);

        let record: RepoRecord = (
            key,
            created_at,
            Utc::now().timestamp_micros() as u64,
            value.clone(),
        );

        txn.set(key_bytes.clone(), rmp_serde::to_vec(&record)?)?;

        let idx_key = self.index_key();
        let mut ids = txn
            .get(&idx_key)?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();

        if !ids.contains(&key) {
            ids.push(key);
            txn.set(idx_key, encode_ids(&ids))?;
        }

        txn.commit().await?;
        Ok(true)
    }

    async fn get(&self, key: RecordId) -> DbResult<RepoRecord> {
        let txn = self.tree.begin()?;
        let key_bytes = prefixed_key(&self.table_name, key);

        let val = txn
            .get(&key_bytes)?
            .ok_or_else(|| DbError::Internal(format!("Key not found: {:?}", key)))?;

        rmp_serde::from_slice(&val).map_err(|e| DbError::Codec(e.to_string()))
    }

    async fn remove(&self, key: RecordId) -> DbResult<bool> {
        let mut txn = self.tree.begin()?;
        let key_bytes = prefixed_key(&self.table_name, key);

        if txn.get(&key_bytes)?.is_none() {
            return Ok(false);
        }

        txn.delete(key_bytes)?;

        let idx_key = self.index_key();
        if let Some(v) = txn.get(&idx_key)? {
            let mut ids = decode_ids(&v);
            ids.retain(|&k| k != key);
            txn.set(idx_key, encode_ids(&ids))?;
        }

        txn.commit().await?;
        Ok(true)
    }

    async fn iter(&self) -> DbResult<Vec<RepoRecord>> {
        let txn = self.tree.begin()?;
        let idx_key = self.index_key();

        let ids = txn
            .get(&idx_key)?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();

        let mut out = Vec::new();
        for id in ids {
            let key = prefixed_key(&self.table_name, id);
            if let Some(val) = txn.get(&key)? {
                out.push(rmp_serde::from_slice(&val)?);
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
#[cfg(not(target_os = "windows"))]
mod tests {
    use super::*;
    use std::fs;
    use tokio::time::{sleep, Duration};
    use serial_test::serial;

    // Generic test function that works with any Store implementation
    async fn run_store_tests(store: &dyn Store) {
        let value1 = InnerValue::Str("hello".to_string());
        let id1 = store.insert(&value1).await.unwrap();
        let retrieved1 = store.get(id1).await.unwrap();
        assert_eq!(retrieved1.3, value1);

        sleep(Duration::from_micros(50)).await;
        let value2 = InnerValue::Str("world".to_string());
        store.set(id1, &value2).await.unwrap();
        let retrieved2 = store.get(id1).await.unwrap();
        assert_eq!(retrieved2.3, value2);
        assert_eq!(retrieved2.1, retrieved1.1);
        assert!(retrieved2.2 > retrieved1.2);

        let _id2 = store.insert(&InnerValue::Int(99)).await.unwrap();
        assert_eq!(store.iter().await.unwrap().len(), 2);

        assert!(store.remove(id1).await.unwrap());
        assert!(store.get(id1).await.is_err());
    }

    #[tokio::test]
    #[serial]
    async fn test_surrealkv_repo_basic() {
        let path = "./test_data/surrealkv_basic";

        {
            let repo = SurrealKVRepo::new(path).unwrap();
            let store = repo.store_get("test_table").await.unwrap();

            run_store_tests(store.as_ref()).await;

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
            let id1 = store1.insert(&value1).await.unwrap();

            // Insert into table2
            let value2 = InnerValue::Str("table2_value".to_string());
            let id2 = store2.insert(&value2).await.unwrap();

            // Verify isolation - each table should have only 1 record
            assert_eq!(store1.iter().await.unwrap().len(), 1);
            assert_eq!(store2.iter().await.unwrap().len(), 1);

            // Verify correct values
            let retrieved1 = store1.get(id1).await.unwrap();
            assert_eq!(retrieved1.3, value1);

            let retrieved2 = store2.get(id2).await.unwrap();
            assert_eq!(retrieved2.3, value2);

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