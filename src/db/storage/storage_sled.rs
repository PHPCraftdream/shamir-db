use super::types::{Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use crate::types::repo_record::RepoRecord;
use crate::types::value::InnerValue;
use async_trait::async_trait;
use chrono::Utc;
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
    async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        let tree = self.tree.clone();
        let inner_value = value.clone();

        spawn_blocking(move || -> DbResult<RecordId> {
            let id = RecordId::new();
            let now = Utc::now().timestamp_micros() as u64;
            let record: RepoRecord = (id, now, now, inner_value);

            let key = id.as_bytes();
            let serialized =
                rmp_serde::to_vec(&record).map_err(|e| DbError::Codec(e.to_string()))?;

            tree.insert(key, serialized)
                .map_err(|e| DbError::Storage(format!("SledDB insert: {}", e)))?;

            // sled is transactional by default, but we might want to flush explicitly for durability
            tree.flush()
                .map_err(|e| DbError::Storage(format!("SledDB flush: {}", e)))?;

            Ok(id)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordId, value: &InnerValue) -> DbResult<bool> {
        let tree = self.tree.clone();
        let inner_value = value.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let key_bytes = key.as_bytes();

            let existing_val = tree
                .get(key_bytes)
                .map_err(|e| DbError::Storage(format!("SledDB get: {}", e)))?;

            let created_at = match existing_val {
                Some(v) => rmp_serde::from_slice::<RepoRecord>(&v)
                    .map(|r| r.1)
                    .unwrap_or_else(|_| Utc::now().timestamp_micros() as u64),
                None => Utc::now().timestamp_micros() as u64,
            };

            let record: RepoRecord = (
                key,
                created_at,
                Utc::now().timestamp_micros() as u64,
                inner_value,
            );

            let serialized =
                rmp_serde::to_vec(&record).map_err(|e| DbError::Codec(e.to_string()))?;

            tree.insert(key_bytes, serialized)
                .map_err(|e| DbError::Storage(format!("SledDB insert: {}", e)))?;

            tree.flush()
                .map_err(|e| DbError::Storage(format!("SledDB flush: {}", e)))?;

            Ok(true)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordId) -> DbResult<RepoRecord> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<RepoRecord> {
            let key_bytes = key.as_bytes();
            let val = tree
                .get(key_bytes)
                .map_err(|e| DbError::Storage(format!("SledDB get: {}", e)))?
                .ok_or_else(|| DbError::NotFound(key.to_string()))?;

            rmp_serde::from_slice(&val).map_err(|e| DbError::Codec(e.to_string()))
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

    async fn iter(&self) -> DbResult<Vec<RepoRecord>> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<Vec<RepoRecord>> {
            let mut out = Vec::new();
            for item in tree.iter() {
                let (_key, val) =
                    item.map_err(|e| DbError::Storage(format!("SledDB iter item: {}", e)))?;
                let record: RepoRecord =
                    rmp_serde::from_slice(&val).map_err(|e| DbError::Codec(e.to_string()))?;
                out.push(record);
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

        let value3 = InnerValue::Int(99);
        let _id2 = store.insert(&value3).await.unwrap();
        let all_records = store.iter().await.unwrap();
        assert_eq!(all_records.len(), 2);
        assert!(all_records.iter().any(|r| r.3 == value2));
        assert!(all_records.iter().any(|r| r.3 == value3));

        assert!(store.remove(id1).await.unwrap());

        // Verify removal
        match store.get(id1).await {
            Err(DbError::NotFound(_)) => { /* Correct */ }
            Ok(_) => panic!("Should have been removed"),
            Err(e) => panic!("Unexpected error: {}", e),
        }

        assert_eq!(store.iter().await.unwrap().len(), 1);
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

        // Verify cross-table isolation (get should fail with NotFound)
        assert!(matches!(store2.get(id1).await, Err(DbError::NotFound(_))));
        assert!(matches!(store1.get(id2).await, Err(DbError::NotFound(_))));

        // Clean up
        repo.store_delete("isolated_table1").await.unwrap();
        repo.store_delete("isolated_table2").await.unwrap();
    }
}
