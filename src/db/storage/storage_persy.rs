use super::types::{PrefixScan, RecordKey, Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use async_trait::async_trait;
use async_stream::stream;
use bytes::Bytes;
use futures::stream::Stream;
use persy::{Persy, PersyId, Config, ByteVec};
use std::path::Path;
use std::pin::Pin;
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

            // Create index for RecordKey -> PersyId mapping
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
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<RecordKey> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;

            let id = RecordId::new();
            let key = RecordKey::copy_from_slice(id.as_bytes());

            let persy_id = tx.insert(&table_name, &*value)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            // Store RecordKey -> PersyId mapping in index
            let key_bytes_index = ByteVec::new(key.to_vec());
            let val = ByteVec::new(persy_id.to_string().into_bytes());
            tx.put(&index_name, key_bytes_index, val)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            tx.prepare().map_err(|e| DbError::Storage(e.to_string()))?
                .commit().map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(key)
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let key_bytes = ByteVec::new(key.to_vec());

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

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<Bytes> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let key_bytes = ByteVec::new(key.to_vec());

            let mut iter = tx.get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            let persy_id_str_bytes = iter.next()
                .ok_or_else(|| DbError::NotFound(format!("{:?}", key)))?;
            let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                .map_err(|e| DbError::Codec(e.to_string()))?;
            let persy_id: PersyId = persy_id_str.parse()
                .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;

            let val = tx.read(&table_name, &persy_id)
                .map_err(|e| DbError::Storage(e.to_string()))?
                .ok_or_else(|| DbError::NotFound(format!("{:?}", key)))?;

            Ok(Bytes::copy_from_slice(&val))
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let key_bytes = ByteVec::new(key.to_vec());

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

    async fn iter(&self) -> DbResult<Vec<(RecordKey, Bytes)>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<Vec<(RecordKey, Bytes)>> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;

            // First: collect all RecordKey -> PersyId mappings
            let mut mappings = Vec::new();
            {
                let mut index_iter = tx.range::<ByteVec, ByteVec, _>(&index_name, ..)
                    .map_err(|e| DbError::Storage(e.to_string()))?;

                while let Some((key_bytes, mut val_iter)) = index_iter.next() {
                    let key = RecordKey::copy_from_slice(key_bytes.as_ref());

                    if let Some(val_bytes) = val_iter.next() {
                        let persy_id_str = String::from_utf8(val_bytes.to_vec())
                            .map_err(|e| DbError::Codec(e.to_string()))?;
                        let persy_id: PersyId = persy_id_str.parse()
                            .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                        mappings.push((key, persy_id));
                    }
                }
            } // index_iter dropped here

            // Second: read all data using collected mappings
            let mut out = Vec::new();
            for (key, persy_id) in mappings {
                let content = tx.read(&table_name, &persy_id)
                    .map_err(|e| DbError::Storage(e.to_string()))?
                    .ok_or_else(|| DbError::NotFound(format!("PersyId not found for key: {:?}", key)))?;
                out.push((key, Bytes::copy_from_slice(&content)));
            }

            Ok(out)
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    fn iter_stream(&self, batch_size: usize) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        Box::pin(stream! {
            let mut mappings: Vec<(RecordKey, PersyId)> = Vec::new();
            let mut collected = false;

            loop {
                // First iteration: collect all mappings
                if !collected {
                    let db_clone = db.clone();
                    let index_name_clone = index_name.clone();

                    let collect_result: DbResult<Vec<_>> = spawn_blocking(move || {
                        let mut tx = db_clone.begin().map_err(|e| DbError::Storage(e.to_string()))?;
                        let mut result = Vec::new();

                        let mut index_iter = tx.range::<ByteVec, ByteVec, _>(&index_name_clone, ..)
                            .map_err(|e| DbError::Storage(e.to_string()))?;

                        while let Some((key_bytes, mut val_iter)) = index_iter.next() {
                            let key = RecordKey::copy_from_slice(key_bytes.as_ref());

                            if let Some(val_bytes) = val_iter.next() {
                                let persy_id_str = String::from_utf8(val_bytes.to_vec())
                                    .map_err(|e| DbError::Codec(e.to_string()))?;
                                let persy_id: PersyId = persy_id_str.parse()
                                    .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                                result.push((key, persy_id));
                            }
                        }

                        Ok(result)
                    })
                    .await
                    .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                    mappings = collect_result?;
                    collected = true;
                }

                // Yield next batch
                if mappings.is_empty() {
                    break;
                }

                let end_idx = batch_size.min(mappings.len());
                let batch_mappings = mappings.drain(..end_idx).collect::<Vec<_>>();

                let db_clone = db.clone();
                let table_name_clone = table_name.clone();

                let batch_result: DbResult<Vec<_>> = spawn_blocking(move || {
                    let mut tx = db_clone.begin().map_err(|e| DbError::Storage(e.to_string()))?;
                    let mut out = Vec::new();

                    for (key, persy_id) in batch_mappings {
                        let content = tx.read(&table_name_clone, &persy_id)
                            .map_err(|e| DbError::Storage(e.to_string()))?
                            .ok_or_else(|| DbError::NotFound(format!("PersyId not found")))?;
                        out.push((key, Bytes::copy_from_slice(&content)));
                    }

                    Ok(out)
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let batch = batch_result?;

                if batch.is_empty() {
                    break;
                }

                yield Ok(batch);
            }
        })
    }
}

// ============================================================================
// PrefixScan implementation for PersyStore
// ============================================================================

#[async_trait]
impl PrefixScan for PersyStore {
    async fn scan_prefix(&self, prefix: Bytes) -> DbResult<Vec<(RecordKey, Bytes)>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<Vec<(RecordKey, Bytes)>> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;

            // Calculate upper bound for prefix scan
            let mut prefix_end = prefix.to_vec();
            if let Some(last_byte) = prefix_end.last_mut() {
                *last_byte = last_byte.wrapping_add(1);
            }

            // Scan the index to find all keys with the given prefix
            let mut mappings = Vec::new();
            {
                let prefix_bv = ByteVec::new(prefix.to_vec());
                let prefix_end_bv = ByteVec::new(prefix_end);

                let mut index_iter = tx
                    .range::<ByteVec, ByteVec, _>(&index_name, prefix_bv..prefix_end_bv)
                    .map_err(|e| DbError::Storage(e.to_string()))?;

                while let Some((key_bytes, mut val_iter)) = index_iter.next() {
                    let key = RecordKey::copy_from_slice(key_bytes.as_ref());

                    if let Some(val_bytes) = val_iter.next() {
                        let persy_id_str = String::from_utf8(val_bytes.to_vec())
                            .map_err(|e| DbError::Codec(e.to_string()))?;
                        let persy_id: PersyId = persy_id_str.parse()
                            .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                        mappings.push((key, persy_id));
                    }
                }
            }

            // Read all data using collected mappings
            let mut out = Vec::new();
            for (key, persy_id) in mappings {
                let content = tx.read(&table_name, &persy_id)
                    .map_err(|e| DbError::Storage(e.to_string()))?
                    .ok_or_else(|| DbError::NotFound(format!("PersyId not found")))?;
                out.push((key, Bytes::copy_from_slice(&content)));
            }

            Ok(out)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        Box::pin(stream! {
            let mut mappings: Vec<(RecordKey, PersyId)> = Vec::new();
            let mut collected = false;
            let mut mapping_offset = 0;

            // Calculate upper bound for prefix
            let mut prefix_end = prefix.to_vec();
            if let Some(last_byte) = prefix_end.last_mut() {
                *last_byte = last_byte.wrapping_add(1);
            }

            loop {
                // First iteration: collect all mappings
                if !collected {
                    let db_clone = db.clone();
                    let index_name_clone = index_name.clone();
                    let prefix_clone = prefix.clone();
                    let prefix_end_clone = prefix_end.clone();

                    let collect_result: DbResult<Vec<_>> = spawn_blocking(move || {
                        let mut tx = db_clone.begin().map_err(|e| DbError::Storage(e.to_string()))?;
                        let mut result = Vec::new();

                        let prefix_bv = ByteVec::new(prefix_clone.to_vec());
                        let prefix_end_bv = ByteVec::new(prefix_end_clone);

                        let mut index_iter = tx
                            .range::<ByteVec, ByteVec, _>(&index_name_clone, prefix_bv..prefix_end_bv)
                            .map_err(|e| DbError::Storage(e.to_string()))?;

                        while let Some((key_bytes, mut val_iter)) = index_iter.next() {
                            let key = RecordKey::copy_from_slice(key_bytes.as_ref());

                            if let Some(val_bytes) = val_iter.next() {
                                let persy_id_str = String::from_utf8(val_bytes.to_vec())
                                    .map_err(|e| DbError::Codec(e.to_string()))?;
                                let persy_id: PersyId = persy_id_str.parse()
                                    .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                                result.push((key, persy_id));
                            }
                        }

                        Ok(result)
                    })
                    .await
                    .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                    mappings = collect_result?;
                    collected = true;
                }

                // Yield next batch
                if mapping_offset >= mappings.len() {
                    break;
                }

                let end_idx = (mapping_offset + batch_size).min(mappings.len());
                let batch_mappings = mappings[mapping_offset..end_idx].to_vec();
                mapping_offset = end_idx;

                let db_clone = db.clone();
                let table_name_clone = table_name.clone();

                let batch_result: DbResult<Vec<_>> = spawn_blocking(move || {
                    let mut tx = db_clone.begin().map_err(|e| DbError::Storage(e.to_string()))?;
                    let mut out = Vec::new();

                    for (key, persy_id) in batch_mappings {
                        let content = tx.read(&table_name_clone, &persy_id)
                            .map_err(|e| DbError::Storage(e.to_string()))?
                            .ok_or_else(|| DbError::NotFound(format!("PersyId not found")))?;
                        out.push((key, Bytes::copy_from_slice(&content)));
                    }

                    Ok(out)
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let batch = batch_result?;

                if batch.is_empty() {
                    break;
                }

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
        let key1 = store1.insert(value1.to_bytes()).await.unwrap();

        let value2 = InnerValue::Str("table2_value".to_string());
        let key2 = store2.insert(value2.to_bytes()).await.unwrap();

        assert_eq!(store1.iter().await.unwrap().len(), 1);
        assert_eq!(store2.iter().await.unwrap().len(), 1);

        let retrieved_bytes1 = store1.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes1).unwrap(), value1);

        let retrieved_bytes2 = store2.get(key2.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes2).unwrap(), value2);

        assert!(matches!(store2.get(key1).await, Err(DbError::NotFound(_))));
        assert!(matches!(store1.get(key2).await, Err(DbError::NotFound(_))));

        repo.store_delete("isolated_table1").await.unwrap();
        repo.store_delete("isolated_table2").await.unwrap();
    }

    #[tokio::test]
    async fn test_persy_prefix_scan() {
        let path = "./test_data/persy_prefix_scan.persy";
        if std::path::Path::new(path).exists() {
            fs::remove_file(path).unwrap();
        }

        let repo = PersyRepo::new(path).unwrap();
        let db = repo.db.clone();

        // Create PersyStore directly to access PrefixScan
        let table_name = "test_table";
        let index_name = format!("{}_idx", table_name);
        let mut tx = db.begin().unwrap();

        tx.create_segment(&table_name).unwrap();
        tx.create_index::<ByteVec, ByteVec>(&index_name, persy::ValueMode::Replace).unwrap();
        tx.prepare().unwrap().commit().unwrap();

        let store = PersyStore {
            db,
            table_name: table_name.to_string(),
            index_name,
        };

        // Insert records with composite keys
        let data = vec![
            (b"country:Russia:Moscow:user1".to_vec(), InnerValue::Str("Alice".to_string())),
            (b"country:Russia:Moscow:user2".to_vec(), InnerValue::Str("Bob".to_string())),
            (b"country:Russia:SPb:user3".to_vec(), InnerValue::Str("Charlie".to_string())),
            (b"country:France:Paris:user4".to_vec(), InnerValue::Str("David".to_string())),
        ];

        for (key, value) in &data {
            store.set(key.clone().into(), value.to_bytes()).await.unwrap();
        }

        // Test prefix scan for "country:Russia:Moscow:"
        let results = store
            .scan_prefix(Bytes::copy_from_slice(b"country:Russia:Moscow:"))
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|(k, _)| k.as_ref() == b"country:Russia:Moscow:user1"));
        assert!(results.iter().any(|(k, _)| k.as_ref() == b"country:Russia:Moscow:user2"));

        // Test prefix scan for "country:Russia:"
        let results_russia = store.scan_prefix(Bytes::copy_from_slice(b"country:Russia:")).await.unwrap();
        assert_eq!(results_russia.len(), 3);

        // Test streaming prefix scan
        let mut stream = store.scan_prefix_stream(Bytes::copy_from_slice(b"country:Russia:"), 2);
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
}
