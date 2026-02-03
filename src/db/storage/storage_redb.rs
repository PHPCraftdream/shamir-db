use super::types::{Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use async_trait::async_trait;
use async_stream::stream;
use bytes::Bytes;
use futures::stream::{Stream};
use redb::{Database, Key, ReadableDatabase, ReadableTable, TableDefinition, TableHandle, Value};
use std::cmp::Ordering;
use std::ops::Bound;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task;

// ============================================================================
// Redb Types & Serialization wrappers
// ============================================================================

// Implement redb::Value for our RecordId key
impl Value for RecordId {
    type SelfType<'a> = Self;
    type AsBytes<'a> = [u8; 16];
    fn fixed_width() -> Option<usize> {
        Some(16)
    }
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        let arr: [u8; 16] = data.try_into().unwrap();
        RecordId(arr)
    }
    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a> {
        value.0
    }
    fn type_name() -> redb::TypeName {
        redb::TypeName::new("RecordId")
    }
}

// Implement redb::Key for our RecordId key
impl Key for RecordId {
    fn compare(data1: &[u8], data2: &[u8]) -> Ordering {
        data1.cmp(data2)
    }
}

// Конвертация ошибок redb в DbError
impl From<redb::Error> for DbError { fn from(err: redb::Error) -> Self { DbError::Storage(err.to_string()) } }
impl From<redb::TransactionError> for DbError { fn from(err: redb::TransactionError) -> Self { DbError::Storage(err.to_string()) } }
impl From<redb::TableError> for DbError { fn from(err: redb::TableError) -> Self { DbError::Storage(err.to_string()) } }
impl From<redb::CommitError> for DbError { fn from(err: redb::CommitError) -> Self { DbError::Storage(err.to_string()) } }
impl From<redb::DatabaseError> for DbError { fn from(err: redb::DatabaseError) -> Self { DbError::Storage(err.to_string()) } }
impl From<redb::StorageError> for DbError { fn from(err: redb::StorageError) -> Self { DbError::Storage(err.to_string()) } }

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
                    write_txn.open_table(TableDefinition::<RecordId, &[u8]>::new(&table_name))?;
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
            let def = TableDefinition::<RecordId, &[u8]>::new(&name);
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
    async fn insert(&self, value: Bytes) -> DbResult<RecordId> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<RecordId> {
            let id = RecordId::new();
            let write_txn = db.begin_write()?;
            {
                let table_def = TableDefinition::<RecordId, &[u8]>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;
                if table.get(id)?.is_some() {
                    return Err(DbError::KeyExists(format!("Key already exists: {:?}", id)));
                }
                table.insert(id, &value[..])?;
            }
            write_txn.commit()?;
            Ok(id)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn set(&self, key: RecordId, value: Bytes) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<bool> {
            let write_txn = db.begin_write()?;
            let created;
            {
                let table_def = TableDefinition::<RecordId, &[u8]>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;
                let old_value = table.insert(key, &value[..])?;
                created = old_value.is_none();
            }
            write_txn.commit()?;
            Ok(created)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn get(&self, key: RecordId) -> DbResult<Bytes> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<Bytes> {
            let read_txn = db.begin_read()?;
            let table_def = TableDefinition::<RecordId, &[u8]>::new(&table_name);
            let table = read_txn.open_table(table_def)?;
            match table.get(key)? {
                Some(guard) => Ok(Bytes::copy_from_slice(guard.value())),
                None => Err(DbError::NotFound(format!("record not found: {:}", key))),
            }
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn remove(&self, key: RecordId) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<bool> {
            let write_txn = db.begin_write()?;
            let removed;
            {
                let table_def = TableDefinition::<RecordId, &[u8]>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;
                removed = table.remove(key)?.is_some();
            }
            write_txn.commit()?;
            Ok(removed)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn iter(&self) -> DbResult<Vec<(RecordId, Bytes)>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<Vec<(RecordId, Bytes)>> {
            let read_txn = db.begin_read()?;
            let table_def = TableDefinition::<RecordId, &[u8]>::new(&table_name);
            let table = read_txn.open_table(table_def)?;
            let mut result = Vec::new();
            for item in table.iter()? {
                let (key, val) = item?;
                result.push((key.value(), Bytes::copy_from_slice(val.value())));
            }
            Ok(result)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    fn iter_stream(&self, batch_size: usize) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordId, Bytes)>, DbError>> + Send>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        Box::pin(stream! {
            let mut last_id: Option<RecordId> = None;

            loop {
                let db_clone = db.clone();
                let table_name_clone = table_name.clone();
                let start_id = last_id;

                let batch: DbResult<Vec<_>> = task::spawn_blocking(move || {
                    let read_txn = db_clone.begin_read()?;
                    let table_def = TableDefinition::<RecordId, &[u8]>::new(&table_name_clone);
                    let table = read_txn.open_table(table_def)?;

                    let range = if let Some(start) = start_id {
                        (Bound::Excluded(start), Bound::Unbounded)
                    } else {
                        (Bound::Unbounded, Bound::Unbounded)
                    };

                    let mut items = Vec::new();
                    for item in table.range(range)?.take(batch_size) {
                        let (key, val) = item?;
                        items.push((key.value(), val.value().to_vec()));
                    }
                    Ok(items)
                })
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

                let batch = batch?;

                if batch.is_empty() {
                    break;
                }

                last_id = batch.last().map(|(id, _)| *id);

                let result_batch: DbResult<Vec<_>> = batch.into_iter()
                    .map(|(id, val)| Ok((id, Bytes::copy_from_slice(&val))))
                    .collect();

                yield result_batch;
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
    use crate::types::value::InnerValue;
    use futures::StreamExt;
    use std::fs;
    use tokio::time::{sleep, Duration};

    async fn run_store_tests(store: &dyn Store) {
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
        let mut expected_ids = Vec::new();
        for i in 0..25 {
            let value = InnerValue::Int(i);
            let id = store.insert(value.to_bytes()).await.unwrap();
            expected_ids.push(id);
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

        // Verify all IDs are present
        for id in &expected_ids {
            assert!(all_records.iter().any(|(rec_id, _)| rec_id == id));
        }
    }
}
