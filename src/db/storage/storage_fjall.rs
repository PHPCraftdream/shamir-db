use super::types::{Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use crate::types::repo_record::RepoRecord;
use crate::types::value::InnerValue;
use async_trait::async_trait;
use chrono::Utc;
use fjall::{Database, Keyspace, KeyspaceCreateOptions};
use std::path::Path;
use std::sync::Arc;

// ============================================================================
// FjallRepo - manages multiple stores (keyspaces)
// ============================================================================

pub struct FjallRepo {
    db: Arc<Database>,
}

impl FjallRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let db = Database::builder(path.as_ref())
            .open()
            .map_err(|e| DbError::Storage(e.to_string()))?;

        Ok(Self {
            db: Arc::new(db),
        })
    }

    fn register_store(&self, table_name: &str) -> DbResult<()> {
        let tables_keyspace = self.db
            .keyspace("__system__", || KeyspaceCreateOptions::default())
            .map_err(|e| DbError::Storage(e.to_string()))?;

        let tables_key = b"__tables__";
        let mut tables = tables_keyspace
            .get(tables_key)
            .map_err(|e| DbError::Storage(e.to_string()))?
            .map(|v| decode_tables(&v))
            .unwrap_or_default();

        if !tables.contains(&table_name.to_string()) {
            tables.push(table_name.to_string());
            tables_keyspace
                .insert(tables_key, encode_tables(&tables))
                .map_err(|e| DbError::Storage(e.to_string()))?;
        }

        Ok(())
    }

    fn unregister_store(&self, table_name: &str) -> DbResult<bool> {
        let tables_keyspace = self.db
            .keyspace("__system__", || KeyspaceCreateOptions::default())
            .map_err(|e| DbError::Storage(e.to_string()))?;

        let tables_key = b"__tables__";
        let existed = if let Some(v) = tables_keyspace.get(tables_key)
            .map_err(|e| DbError::Storage(e.to_string()))? {
            let mut tables = decode_tables(&v);
            let existed = tables.contains(&table_name.to_string());
            tables.retain(|t| t != table_name);
            tables_keyspace
                .insert(tables_key, encode_tables(&tables))
                .map_err(|e| DbError::Storage(e.to_string()))?;
            existed
        } else {
            false
        };

        Ok(existed)
    }

    pub async fn store_get_by_name(&self, name: &str) -> DbResult<Arc<dyn Store>> {
        let keyspace = self.db
            .keyspace(name, || KeyspaceCreateOptions::default())
            .map_err(|e| DbError::Storage(e.to_string()))?;

        self.register_store(name)?;

        Ok(Arc::new(FjallStore {
            keyspace: keyspace,
            table_name: name.to_string(),
        }))
    }

    pub async fn store_delete_by_name(&self, name: &str) -> DbResult<bool> {
        let keyspace = self.db
            .keyspace(name, || KeyspaceCreateOptions::default())
            .map_err(|e| DbError::Storage(e.to_string()))?;

        let idx_key = format!("__index__:{}:__keys__", name).into_bytes();

        let ids = keyspace
            .get(&idx_key)
            .map_err(|e| DbError::Storage(e.to_string()))?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();

        // Delete all records (if any)
        for id in &ids {
            let key = record_key(name, *id);
            keyspace.remove(&key)
                .map_err(|e| DbError::Storage(e.to_string()))?;
        }

        // Delete index (if exists)
        if !ids.is_empty() {
            keyspace.remove(&idx_key)
                .map_err(|e| DbError::Storage(e.to_string()))?;
        }

        // Remove table from global tables list
        self.unregister_store(name)
    }
}

#[async_trait]
impl Repo for FjallRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        self.store_get_by_name(name.as_ref()).await
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        self.store_delete_by_name(name.as_ref()).await
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let tables_keyspace = self.db
            .keyspace("__system__", || KeyspaceCreateOptions::default())
            .map_err(|e| DbError::Storage(e.to_string()))?;

        let tables_key = b"__tables__";
        let tables = tables_keyspace
            .get(tables_key)
            .map_err(|e| DbError::Storage(e.to_string()))?
            .map(|v| decode_tables(&v))
            .unwrap_or_default();

        Ok(tables)
    }
}

// ============================================================================
// FjallStore - individual store (keyspace)
// ============================================================================

pub struct FjallStore {
    keyspace: Keyspace,
    table_name: String,
}

unsafe impl Send for FjallStore {}
unsafe impl Sync for FjallStore {}

impl FjallStore {
    fn record_key(&self, id: RecordId) -> Vec<u8> {
        record_key(&self.table_name, id)
    }

    fn index_key(&self) -> Vec<u8> {
        format!("__index__:{}:__keys__", self.table_name).into_bytes()
    }
}

#[async_trait]
impl Store for FjallStore {
    async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        let id = RecordId::new();
        let now = Utc::now().timestamp_micros() as u64;
        let record: RepoRecord = (id, now, now, value.clone());

        let key = self.record_key(id);

        // Check if key already exists
        if self.keyspace.get(&key)
            .map_err(|e| DbError::Storage(e.to_string()))?
            .is_some() {
            return Err(DbError::Internal(format!("Key already exists: {:?}", id)));
        }

        let serialized = rmp_serde::to_vec(&record)
            .map_err(|e| DbError::Codec(e.to_string()))?;

        self.keyspace.insert(&key, serialized)
            .map_err(|e| DbError::Storage(e.to_string()))?;

        // Update index
        let idx_key = self.index_key();
        let mut ids = self.keyspace
            .get(&idx_key)
            .map_err(|e| DbError::Storage(e.to_string()))?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();

        ids.push(id);
        self.keyspace.insert(&idx_key, encode_ids(&ids))
            .map_err(|e| DbError::Storage(e.to_string()))?;

        Ok(id)
    }

    async fn set(&self, key: RecordId, value: &InnerValue) -> DbResult<bool> {
        let key_bytes = self.record_key(key);

        let existing = self.keyspace
            .get(&key_bytes)
            .map_err(|e| DbError::Storage(e.to_string()))?
            .and_then(|v| rmp_serde::from_slice::<RepoRecord>(&v).ok());

        let created_at = existing.map_or(Utc::now().timestamp_micros() as u64, |r| r.1);

        let record: RepoRecord = (
            key,
            created_at,
            Utc::now().timestamp_micros() as u64,
            value.clone(),
        );

        let serialized = rmp_serde::to_vec(&record)
            .map_err(|e| DbError::Codec(e.to_string()))?;

        self.keyspace.insert(&key_bytes, serialized)
            .map_err(|e| DbError::Storage(e.to_string()))?;

        // Update index if new key
        let idx_key = self.index_key();
        let mut ids = self.keyspace
            .get(&idx_key)
            .map_err(|e| DbError::Storage(e.to_string()))?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();

        if !ids.contains(&key) {
            ids.push(key);
            self.keyspace.insert(&idx_key, encode_ids(&ids))
                .map_err(|e| DbError::Storage(e.to_string()))?;
        }

        Ok(true)
    }

    async fn get(&self, key: RecordId) -> DbResult<RepoRecord> {
        let key_bytes = self.record_key(key);

        let val = self.keyspace
            .get(&key_bytes)
            .map_err(|e| DbError::Storage(e.to_string()))?
            .ok_or_else(|| DbError::Internal(format!("Key not found: {:?}", key)))?;

        rmp_serde::from_slice(&val).map_err(|e| DbError::Codec(e.to_string()))
    }

    async fn remove(&self, key: RecordId) -> DbResult<bool> {
        let key_bytes = self.record_key(key);

        if self.keyspace.get(&key_bytes)
            .map_err(|e| DbError::Storage(e.to_string()))?
            .is_none() {
            return Ok(false);
        }

        self.keyspace.remove(&key_bytes)
            .map_err(|e| DbError::Storage(e.to_string()))?;

        // Update index
        let idx_key = self.index_key();
        if let Some(v) = self.keyspace.get(&idx_key)
            .map_err(|e| DbError::Storage(e.to_string()))? {
            let mut ids = decode_ids(&v);
            ids.retain(|&k| k != key);
            self.keyspace.insert(&idx_key, encode_ids(&ids))
                .map_err(|e| DbError::Storage(e.to_string()))?;
        }

        Ok(true)
    }

    async fn iter(&self) -> DbResult<Vec<RepoRecord>> {
        let idx_key = self.index_key();

        let ids = self.keyspace
            .get(&idx_key)
            .map_err(|e| DbError::Storage(e.to_string()))?
            .map(|v| decode_ids(&v))
            .unwrap_or_default();

        let mut out = Vec::new();
        for id in ids {
            let key = self.record_key(id);
            if let Some(val) = self.keyspace.get(&key)
                .map_err(|e| DbError::Storage(e.to_string()))? {
                let record: RepoRecord = rmp_serde::from_slice(&val)
                    .map_err(|e| DbError::Codec(e.to_string()))?;
                out.push(record);
            }
        }
        Ok(out)
    }
}

// ============================================================================
// Helper functions
// ============================================================================

fn record_key(table_name: &str, id: RecordId) -> Vec<u8> {
    let mut key = Vec::with_capacity(table_name.len() + 1 + 16);
    key.extend_from_slice(table_name.as_bytes());
    key.push(b':');
    key.extend_from_slice(id.as_bytes());
    key
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
mod tests {
    use super::*;
    use std::fs;
    use tokio::time::{sleep, Duration};

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
    async fn test_fjall_repo_basic() {
        let path = "./test_data/fjall_repo_basic";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = FjallRepo::new(path).unwrap();
        let store = repo.store_get("test_table").await.unwrap();

        run_store_tests(store.as_ref()).await;

        assert!(repo.store_delete("test_table").await.unwrap());
    }

    #[tokio::test]
    async fn test_fjall_repo_list_stores() {
        let path = "./test_data/fjall_repo_list";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = FjallRepo::new(path).unwrap();

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
    }

    #[tokio::test]
    async fn test_fjall_repo_store_isolation() {
        let path = "./test_data/fjall_repo_isolation";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = FjallRepo::new(path).unwrap();

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
    }
}