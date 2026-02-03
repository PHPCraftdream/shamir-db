use super::types::{Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use async_trait::async_trait;
use bytes::Bytes;
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
    async fn insert(&self, value: Bytes) -> DbResult<RecordId> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<RecordId> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;

            let id = RecordId::new();
            let persy_id = tx.insert(&table_name, &*value)
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

    async fn set(&self, key: RecordId, value: Bytes) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let key_bytes = ByteVec::new(key.as_bytes().to_vec());

            // Find PersyId from index
            let mut iter = tx.get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            let created = if let Some(persy_id_str_bytes) = iter.next() {
                // Existing record - update
                let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                    .map_err(|e| DbError::Codec(e.to_string()))?;
                let persy_id: PersyId = persy_id_str.parse()
                    .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;

                tx.update(&table_name, &persy_id, &*value)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
                false
            } else {
                // New record - insert and update index
                let persy_id = tx.insert(&table_name, &*value)
                    .map_err(|e| DbError::Storage(e.to_string()))?;

                let val = ByteVec::new(persy_id.to_string().into_bytes());
                tx.put(&index_name, key_bytes, val)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
                true
            };

            tx.prepare().map_err(|e| DbError::Storage(e.to_string()))?
                .commit().map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(created)
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordId) -> DbResult<Bytes> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<Bytes> {
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

            Ok(Bytes::copy_from_slice(&val))
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

    async fn iter(&self) -> DbResult<Vec<(RecordId, Bytes)>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<Vec<(RecordId, Bytes)>> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;

            // First: collect all RecordId -> PersyId mappings
            let mut mappings = Vec::new();
            {
                let mut index_iter = tx.range::<ByteVec, ByteVec, _>(&index_name, ..)
                    .map_err(|e| DbError::Storage(e.to_string()))?;

                while let Some((key_bytes, mut val_iter)) = index_iter.next() {
                    let record_id = RecordId(key_bytes.as_ref().try_into().map_err(|_| {
                        DbError::Internal("Failed to convert key to RecordId".to_string())
                    })?);

                    if let Some(val_bytes) = val_iter.next() {
                        let persy_id_str = String::from_utf8(val_bytes.to_vec())
                            .map_err(|e| DbError::Codec(e.to_string()))?;
                        let persy_id: PersyId = persy_id_str.parse()
                            .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                        mappings.push((record_id, persy_id));
                    }
                }
            } // index_iter dropped here

            // Second: read all data using collected mappings
            let mut out = Vec::new();
            for (record_id, persy_id) in mappings {
                let content = tx.read(&table_name, &persy_id)
                    .map_err(|e| DbError::Storage(e.to_string()))?
                    .ok_or_else(|| DbError::NotFound(format!("PersyId not found for record: {}", record_id)))?;
                out.push((record_id, Bytes::copy_from_slice(&content)));
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
    async fn test_persy_repo_basic() {
        let path = "./test_data/persy_repo_basic.persy";
        if std::path::Path::new(path).exists() {
            fs::remove_file(path).unwrap();
        }

        let repo = PersyRepo::new(path).unwrap();
        let store = repo.store_get("test_table").await.unwrap();

        run_store_tests(store).await;

        assert!(repo.store_delete("test_table").await.unwrap());
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
        let id1 = store1.insert(value1.to_bytes()).await.unwrap();

        let value2 = InnerValue::Str("table2_value".to_string());
        let id2 = store2.insert(value2.to_bytes()).await.unwrap();

        assert_eq!(store1.iter().await.unwrap().len(), 1);
        assert_eq!(store2.iter().await.unwrap().len(), 1);

        let retrieved_bytes1 = store1.get(id1).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes1).unwrap(), value1);

        let retrieved_bytes2 = store2.get(id2).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes2).unwrap(), value2);

        assert!(matches!(store2.get(id1).await, Err(DbError::NotFound(_))));
        assert!(matches!(store1.get(id2).await, Err(DbError::NotFound(_))));

        repo.store_delete("isolated_table1").await.unwrap();
        repo.store_delete("isolated_table2").await.unwrap();
    }
}