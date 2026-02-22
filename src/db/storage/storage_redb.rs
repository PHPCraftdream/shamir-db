use super::types::{RecordKey, Repo, Store};
use crate::db::{DbError, DbResult};
use crate::types::record_id::RecordId;
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition, TableHandle};
use std::ops::Bound;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task;

// ============================================================================
// Redb Types & Serialization wrappers
// ============================================================================

// Конвертация ошибок redb в DbError
impl From<redb::Error> for DbError {
    fn from(err: redb::Error) -> Self {
        DbError::Storage(err.to_string())
    }
}
impl From<redb::TransactionError> for DbError {
    fn from(err: redb::TransactionError) -> Self {
        DbError::Storage(err.to_string())
    }
}
impl From<redb::TableError> for DbError {
    fn from(err: redb::TableError) -> Self {
        DbError::Storage(err.to_string())
    }
}
impl From<redb::CommitError> for DbError {
    fn from(err: redb::CommitError) -> Self {
        DbError::Storage(err.to_string())
    }
}
impl From<redb::DatabaseError> for DbError {
    fn from(err: redb::DatabaseError) -> Self {
        DbError::Storage(err.to_string())
    }
}
impl From<redb::StorageError> for DbError {
    fn from(err: redb::StorageError) -> Self {
        DbError::Storage(err.to_string())
    }
}

// ============================================================================
// RedbRepo - manages database connection and tables
// ============================================================================

pub struct RedbRepo {
    db: Arc<Database>,
}

impl RedbRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| DbError::Storage(format!("Failed to create dir: {}", e)))?;
            }
        }
        let db = Database::create(path)?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl Repo for RedbRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let table_name = name.as_ref().to_string();
        let db = self.db.clone();

        task::spawn_blocking(move || -> DbResult<Arc<dyn Store>> {
            let write_txn = db.begin_write()?;
            {
                // Scope to ensure the table handle is dropped before commit
                let _table =
                    write_txn.open_table(TableDefinition::<&[u8], &[u8]>::new(&table_name))?;
            }
            write_txn.commit()?;
            Ok(Arc::new(RedbStore { db, table_name }))
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let name = name.as_ref().to_string();
        let db = self.db.clone();
        task::spawn_blocking(move || -> DbResult<bool> {
            let write_txn = db.begin_write()?;
            let def = TableDefinition::<&[u8], &[u8]>::new(&name);
            let deleted = write_txn.delete_table(def)?;
            write_txn.commit()?;
            Ok(deleted)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let db = self.db.clone();
        task::spawn_blocking(move || -> DbResult<Vec<String>> {
            let read_txn = db.begin_read()?;
            let tables = read_txn
                .list_tables()?
                .map(|t| t.name().to_string())
                .collect();
            Ok(tables)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }
}

// ============================================================================
// RedbStore - individual store implementation
// ============================================================================

pub struct RedbStore {
    db: Arc<Database>,
    table_name: String,
}

#[async_trait]
impl Store for RedbStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<RecordKey> {
            let id = RecordId::new();
            let key = RecordKey::copy_from_slice(id.as_bytes());

            let write_txn = db.begin_write()?;
            {
                let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;

                // Check if key exists
                if table.get(&key[..])?.is_some() {
                    return Err(DbError::KeyExists(format!("Key already exists: {:?}", key)));
                }

                table.insert(&key[..], &value[..])?;
            }
            write_txn.commit()?;
            Ok(key)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<bool> {
            let write_txn = db.begin_write()?;
            let created;
            {
                let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;
                let old_value = table.insert(&key[..], &value[..])?;
                created = old_value.is_none();
            }
            write_txn.commit()?;
            Ok(created)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<Bytes> {
            let read_txn = db.begin_read()?;
            let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name);
            let table = read_txn.open_table(table_def)?;
            match table.get(&key[..])? {
                Some(guard) => Ok(Bytes::copy_from_slice(guard.value())),
                None => Err(DbError::NotFound(format!("record not found: {:?}", key))),
            }
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<bool> {
            let write_txn = db.begin_write()?;
            let removed;
            {
                let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;
                removed = table.remove(&key[..])?.is_some();
            }
            write_txn.commit()?;
            Ok(removed)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            loop {
                let db_clone = db.clone();
                let table_name_clone = table_name.clone();
                let start_key = last_key;

                let batch: DbResult<Vec<_>> = task::spawn_blocking(move || {
                    let read_txn = db_clone.begin_read()?;
                    let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name_clone);
                    let table = read_txn.open_table(table_def)?;

                    let range: (Bound<&[u8]>, Bound<&[u8]>) = if let Some(ref start) = start_key {
                        (Bound::Excluded(start.as_slice()), Bound::Unbounded)
                    } else {
                        (Bound::Unbounded, Bound::Unbounded)
                    };

                    let mut items = Vec::new();
                    for item in table.range::<&[u8]>(range)?.take(batch_size) {
                        let (key, val) = item?;
                        items.push((Bytes::copy_from_slice(key.value()), Bytes::copy_from_slice(val.value())));
                    }
                    Ok(items)
                })
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

                let batch = batch?;

                if batch.is_empty() {
                    break;
                }

                last_key = batch.last().map(|(k, _)| k.to_vec());
                yield Ok(batch);
            }
        })
    }

    async fn scan_prefix(&self, prefix: Bytes) -> DbResult<Vec<(RecordKey, Bytes)>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        task::spawn_blocking(move || -> DbResult<Vec<(RecordKey, Bytes)>> {
            let read_txn = db.begin_read()?;
            let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name);
            let table = read_txn.open_table(table_def)?;

            let mut result = Vec::new();

            // Calculate the upper bound for prefix scan
            // We need to find the next possible prefix to use as exclusive upper bound
            let mut prefix_end = prefix.to_vec();
            if let Some(last_byte) = prefix_end.last_mut() {
                *last_byte = last_byte.wrapping_add(1);
            } else {
                // Empty prefix - return everything
                for item in table.iter()? {
                    let (key, val) = item?;
                    result.push((
                        Bytes::copy_from_slice(key.value()),
                        Bytes::copy_from_slice(val.value()),
                    ));
                }
                return Ok(result);
            }

            // Use range with prefix bounds
            let range = (
                Bound::Included(prefix.as_ref()),
                Bound::Excluded(prefix_end.as_slice()),
            );

            for item in table.range::<&[u8]>(range)? {
                let (key, val) = item?;
                result.push((
                    Bytes::copy_from_slice(key.value()),
                    Bytes::copy_from_slice(val.value()),
                ));
            }

            Ok(result)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            // Calculate upper bound for prefix
            let mut prefix_end = prefix.to_vec();
            if let Some(last_byte) = prefix_end.last_mut() {
                *last_byte = last_byte.wrapping_add(1);
            }

            loop {
                let db_clone = db.clone();
                let table_name_clone = table_name.clone();
                let start_key = last_key.clone();
                let prefix_clone = prefix.clone();
                let prefix_end_clone = prefix_end.clone();

                let batch: DbResult<Vec<_>> = task::spawn_blocking(move || {
                    let read_txn = db_clone.begin_read()?;
                    let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name_clone);
                    let table = read_txn.open_table(table_def)?;

                    let range: (Bound<&[u8]>, Bound<&[u8]>) = if let Some(ref start) = start_key {
                        (Bound::Excluded(start.as_slice()), Bound::Excluded(prefix_end_clone.as_slice()))
                    } else {
                        (Bound::Included(prefix_clone.as_ref()), Bound::Excluded(prefix_end_clone.as_slice()))
                    };

                    let mut items = Vec::new();
                    for item in table.range::<&[u8]>(range)?.take(batch_size) {
                        let (key, val) = item?;
                        items.push((Bytes::copy_from_slice(key.value()), Bytes::copy_from_slice(val.value())));
                    }

                    Ok(items)
                })
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

                let batch = batch?;

                if batch.is_empty() {
                    break;
                }

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
    #![allow(deprecated)]

    use super::super::types::collect_stream;
    use super::*;
    use crate::types::record_id::RecordId;
    use crate::types::value::InnerValue;
    use futures::StreamExt;
    use std::fs;
    use tokio::time::{sleep, Duration};

    async fn run_store_tests(store: &dyn Store) {
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
        let all_records = collect_stream(store.iter_stream(1000)).await.unwrap();
        assert_eq!(all_records.len(), 3);
        assert!(all_records.iter().any(|(k, _)| *k == key1));
        assert!(all_records
            .iter()
            .any(|(_, bytes)| InnerValue::from_bytes(bytes.clone()).unwrap() == value4));

        // Test remove
        assert!(store.remove(key1.clone()).await.unwrap());
        assert!(store.get(key1.clone()).await.is_err());
        assert!(!store.remove(key1).await.unwrap()); // Already removed

        let all_records_after_remove = collect_stream(store.iter_stream(1000)).await.unwrap();
        assert_eq!(all_records_after_remove.len(), 2);
    }

    #[tokio::test]
    async fn test_redb_repo_basic() {
        let path = "./test_data/redb_repo_basic/db.redb";
        if let Some(parent) = Path::new(path).parent() {
            if parent.exists() {
                fs::remove_dir_all(parent).unwrap();
            }
        }

        let repo = RedbRepo::new(path).unwrap();
        let store = repo.store_get("test_table").await.unwrap();

        run_store_tests(store.as_ref()).await;

        assert!(repo.store_delete("test_table").await.unwrap());
    }

    #[tokio::test]
    async fn test_redb_iter_stream() {
        let path = "./test_data/redb_iter_stream/db.redb";
        if let Some(parent) = Path::new(path).parent() {
            if parent.exists() {
                fs::remove_dir_all(parent).unwrap();
            }
        }

        let repo = RedbRepo::new(path).unwrap();
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

    #[tokio::test]
    async fn test_redb_prefix_scan() {
        let path = "./test_data/redb_prefix_scan/db.redb";
        if let Some(parent) = Path::new(path).parent() {
            if parent.exists() {
                fs::remove_dir_all(parent).unwrap();
            }
        }

        let repo = RedbRepo::new(path).unwrap();
        let db = repo.db.clone();

        // Create RedbStore directly to access PrefixScan
        let table_name = "test_table";
        let write_txn = db.begin_write().unwrap();
        {
            let _table = write_txn
                .open_table(TableDefinition::<&[u8], &[u8]>::new(table_name))
                .unwrap();
        }
        write_txn.commit().unwrap();

        let store = RedbStore {
            db,
            table_name: table_name.to_string(),
        };

        // Insert records with composite keys
        let data = vec![
            (
                b"country:Russia:Moscow:user1".to_vec(),
                InnerValue::Str("Alice".to_string()),
            ),
            (
                b"country:Russia:Moscow:user2".to_vec(),
                InnerValue::Str("Bob".to_string()),
            ),
            (
                b"country:Russia:SPb:user3".to_vec(),
                InnerValue::Str("Charlie".to_string()),
            ),
            (
                b"country:France:Paris:user4".to_vec(),
                InnerValue::Str("David".to_string()),
            ),
            (
                b"country:France:Lyon:user5".to_vec(),
                InnerValue::Str("Eve".to_string()),
            ),
        ];

        for (key, value) in &data {
            store
                .set(key.clone().into(), value.to_bytes())
                .await
                .unwrap();
        }

        // Test prefix scan for "country:Russia:Moscow:"
        let results = store
            .scan_prefix(Bytes::copy_from_slice(b"country:Russia:Moscow:"))
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .any(|(k, _)| k.as_ref() == b"country:Russia:Moscow:user1"));
        assert!(results
            .iter()
            .any(|(k, _)| k.as_ref() == b"country:Russia:Moscow:user2"));

        // Test prefix scan for "country:Russia:"
        let results_russia = store
            .scan_prefix(Bytes::copy_from_slice(b"country:Russia:"))
            .await
            .unwrap();
        assert_eq!(results_russia.len(), 3);

        // Test prefix scan for "country:France:"
        let results_france = store
            .scan_prefix(Bytes::copy_from_slice(b"country:France:"))
            .await
            .unwrap();
        assert_eq!(results_france.len(), 2);

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
