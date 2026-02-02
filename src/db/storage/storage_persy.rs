use super::types::{Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use crate::types::repo_record::RepoRecord;
use crate::types::value::InnerValue;
use async_trait::async_trait;
use chrono::Utc;
use persy::{Persy, PersyId, Config, ByteVec};
use std::path::Path;
use std::sync::Arc;
use tokio::task::spawn_blocking;

// ============================================================================
// PersyRepo - manages multiple stores (segments)
// ============================================================================

#[derive(Clone)]
pub struct PersyRepo {
    db: Arc<Persy>,
}

impl PersyRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        Persy::create(path.as_ref()).map_err(|e| DbError::Storage(e.to_string()))?;
        let db = Persy::open(path.as_ref(), Config::default()).map_err(|e| DbError::Storage(e.to_string()))?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl Repo for PersyRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();
        let index_name = format!("{}_idx", table_name);

        spawn_blocking(move || -> DbResult<()> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;

            // Create segment if it doesn't exist
            if !tx.exists_segment(&table_name).map_err(|e| DbError::Storage(e.to_string()))? {
                tx.create_segment(&table_name).map_err(|e| DbError::Storage(e.to_string()))?;
            }

            // Create index for RecordId -> PersyId mapping
            if !tx.exists_index(&index_name).map_err(|e| DbError::Storage(e.to_string()))? {
                tx.create_index::<ByteVec, ByteVec>(&index_name, persy::ValueMode::Replace)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
            }

            tx.prepare().map_err(|e| DbError::Storage(e.to_string()))?
                .commit().map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(())
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))??;

        let store = PersyStore {
            db: self.db.clone(),
            table_name: name.as_ref().to_string(),
            index_name: format!("{}_idx", name.as_ref()),
        };
        Ok(Arc::new(store))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();
        let index_name = format!("{}_idx", table_name);

        spawn_blocking(move || -> DbResult<bool> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;

            if tx.exists_index(&index_name).map_err(|e| DbError::Storage(e.to_string()))? {
                tx.drop_index(&index_name).map_err(|e| DbError::Storage(e.to_string()))?;
            }

            if tx.exists_segment(&table_name).map_err(|e| DbError::Storage(e.to_string()))? {
                tx.drop_segment(&table_name).map_err(|e| DbError::Storage(e.to_string()))?;
            }

            tx.prepare().map_err(|e| DbError::Storage(e.to_string()))?
                .commit().map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(true)
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let db = self.db.clone();
        spawn_blocking(move || -> DbResult<Vec<String>> {
            let segments = db.list_segments().map_err(|e| DbError::Storage(e.to_string()))?;
            let names: Vec<String> = segments
                .into_iter()
                .map(|(name, _id)| name)
                .filter(|name| !name.ends_with("_idx"))
                .collect();
            Ok(names)
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }
}

// ============================================================================
// PersyStore - individual store (segment)
// ============================================================================

pub struct PersyStore {
    db: Arc<Persy>,
    table_name: String,
    index_name: String,
}

unsafe impl Send for PersyStore {}
unsafe impl Sync for PersyStore {}

#[async_trait]
impl Store for PersyStore {
    async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();
        let inner_value = value.clone();

        spawn_blocking(move || -> DbResult<RecordId> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;

            let id = RecordId::new();
            let now = Utc::now().timestamp_micros() as u64;
            let record: RepoRecord = (id, now, now, inner_value);
            let serialized = rmp_serde::to_vec(&record).map_err(|e| DbError::Codec(e.to_string()))?;

            let persy_id = tx.insert(&table_name, &serialized)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            // Store RecordId -> PersyId mapping in index
            let key = ByteVec::new(id.as_bytes().to_vec());
            let val = ByteVec::new(persy_id.to_string().into_bytes());
            tx.put(&index_name, key, val)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            tx.prepare().map_err(|e| DbError::Storage(e.to_string()))?
                .commit().map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(id)
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordId, value: &InnerValue) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();
        let inner_value = value.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let key_bytes = ByteVec::new(key.as_bytes().to_vec());

            // Find PersyId from index
            let mut iter = tx.get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            let persy_id_str_bytes = iter.next()
                .ok_or_else(|| DbError::NotFound(key.to_string()))?;
            let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                .map_err(|e| DbError::Codec(e.to_string()))?;
            let persy_id: PersyId = persy_id_str.parse()
                .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;

            let existing_val = tx.read(&table_name, &persy_id)
                .map_err(|e| DbError::Storage(e.to_string()))?
                .ok_or_else(|| DbError::NotFound(key.to_string()))?;

            let created_at = rmp_serde::from_slice::<RepoRecord>(&existing_val)
                .map(|r| r.1)
                .unwrap_or_else(|_| Utc::now().timestamp_micros() as u64);

            let record: RepoRecord = (
                key,
                created_at,
                Utc::now().timestamp_micros() as u64,
                inner_value,
            );

            let serialized = rmp_serde::to_vec(&record).map_err(|e| DbError::Codec(e.to_string()))?;
            tx.update(&table_name, &persy_id, &serialized)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            tx.prepare().map_err(|e| DbError::Storage(e.to_string()))?
                .commit().map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(true)
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordId) -> DbResult<RepoRecord> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<RepoRecord> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let key_bytes = ByteVec::new(key.as_bytes().to_vec());

            let mut iter = tx.get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            let persy_id_str_bytes = iter.next()
                .ok_or_else(|| DbError::NotFound(key.to_string()))?;
            let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                .map_err(|e| DbError::Codec(e.to_string()))?;
            let persy_id: PersyId = persy_id_str.parse()
                .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;

            let val = tx.read(&table_name, &persy_id)
                .map_err(|e| DbError::Storage(e.to_string()))?
                .ok_or_else(|| DbError::NotFound(key.to_string()))?;

            rmp_serde::from_slice(&val).map_err(|e| DbError::Codec(e.to_string()))
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn remove(&self, key: RecordId) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let key_bytes = ByteVec::new(key.as_bytes().to_vec());

            let mut iter = tx.get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            if let Some(persy_id_str_bytes) = iter.next() {
                let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                    .map_err(|e| DbError::Codec(e.to_string()))?;
                let persy_id: PersyId = persy_id_str.parse()
                    .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;

                tx.delete(&table_name, &persy_id)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
                tx.remove(&index_name, key_bytes, None::<ByteVec>)
                    .map_err(|e| DbError::Storage(e.to_string()))?;

                tx.prepare().map_err(|e| DbError::Storage(e.to_string()))?
                    .commit().map_err(|e| DbError::Storage(e.to_string()))?;
                Ok(true)
            } else {
                Ok(false)
            }
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn iter(&self) -> DbResult<Vec<RepoRecord>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        spawn_blocking(move || -> DbResult<Vec<RepoRecord>> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let mut out = Vec::new();

            for (_id, content) in tx.scan(&table_name)
                .map_err(|e| DbError::Storage(e.to_string()))? {
                if let Ok(record) = rmp_serde::from_slice::<RepoRecord>(&content) {
                    out.push(record);
                }
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

        match store.get(id1).await {
            Err(DbError::NotFound(_)) => { /* Correct */ }
            Ok(_) => panic!("Should have been removed"),
            Err(e) => panic!("Unexpected error: {}", e),
        }

        assert_eq!(store.iter().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_persy_repo_basic() {
        let path = "./test_data/persy_repo_basic.persy";
        if std::path::Path::new(path).exists() {
            fs::remove_file(path).unwrap();
        }

        let repo = PersyRepo::new(path).unwrap();
        let store = repo.store_get("test_table").await.unwrap();

        run_store_tests(store).await;
    }

    #[tokio::test]
    async fn test_persy_repo_list_stores() {
        let path = "./test_data/persy_repo_list.persy";
        if std::path::Path::new(path).exists() {
            fs::remove_file(path).unwrap();
        }

        let repo = PersyRepo::new(path).unwrap();

        let _store1 = repo.store_get("table1").await.unwrap();

        let tables = repo.stores_list().await.unwrap();
        assert_eq!(tables.len(), 1);
        assert!(tables.contains(&"table1".to_string()));

        let _store2 = repo.store_get("table2").await.unwrap();

        let tables = repo.stores_list().await.unwrap();
        assert_eq!(tables.len(), 2);
        assert!(tables.contains(&"table1".to_string()));
        assert!(tables.contains(&"table2".to_string()));

        assert!(repo.store_delete("table1").await.unwrap());
        let tables = repo.stores_list().await.unwrap();
        assert_eq!(tables.len(), 1);
        assert!(!tables.contains(&"table1".to_string()));
        assert!(tables.contains(&"table2".to_string()));
    }

    #[tokio::test]
    async fn test_persy_repo_store_isolation() {
        let path = "./test_data/persy_repo_isolation.persy";
        if std::path::Path::new(path).exists() {
            fs::remove_file(path).unwrap();
        }

        let repo = PersyRepo::new(path).unwrap();

        let store1 = repo.store_get("isolated_table1").await.unwrap();
        let store2 = repo.store_get("isolated_table2").await.unwrap();

        let value1 = InnerValue::Str("table1_value".to_string());
        let id1 = store1.insert(&value1).await.unwrap();

        let value2 = InnerValue::Str("table2_value".to_string());
        let id2 = store2.insert(&value2).await.unwrap();

        assert_eq!(store1.iter().await.unwrap().len(), 1);
        assert_eq!(store2.iter().await.unwrap().len(), 1);

        let retrieved1 = store1.get(id1).await.unwrap();
        assert_eq!(retrieved1.3, value1);

        let retrieved2 = store2.get(id2).await.unwrap();
        assert_eq!(retrieved2.3, value2);

        assert!(matches!(store2.get(id1).await, Err(DbError::NotFound(_))));
        assert!(matches!(store1.get(id2).await, Err(DbError::NotFound(_))));

        repo.store_delete("isolated_table1").await.unwrap();
        repo.store_delete("isolated_table2").await.unwrap();
    }
}