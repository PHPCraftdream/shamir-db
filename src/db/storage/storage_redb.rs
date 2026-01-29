use super::types::{Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use crate::types::repo_record::RepoRecord;
use crate::types::value::InnerValue;
use async_trait::async_trait;
use chrono::Utc;
// Добавлен TableHandle для доступа к .name()
use redb::{
    Database, Key, ReadableDatabase, ReadableTable, TableDefinition, TableHandle, TypeName, Value,
};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::Path;
use std::sync::Arc;
use tokio::task;

// ============================================================================
// Redb Types & Serialization wrappers
// ============================================================================

#[derive(Serialize, Deserialize, Debug)]
struct StorableRepoRecord(RepoRecord);

impl Value for StorableRepoRecord {
    type SelfType<'a> = Self;
    type AsBytes<'a> = Vec<u8>;
    fn fixed_width() -> Option<usize> { None }
    fn from_bytes<'a>(data: &'a [u8]) -> Self where Self: 'a {
        rmp_serde::from_slice(data).expect("Failed to deserialize StorableRepoRecord")
    }
    fn as_bytes<'a, 'b: 'a>(value: &'a Self) -> Self::AsBytes<'a> {
        rmp_serde::to_vec(value).expect("Failed to serialize StorableRepoRecord")
    }
    fn type_name() -> TypeName { TypeName::new("StorableRepoRecord") }
}

impl Value for RecordId {
    type SelfType<'a> = Self;
    type AsBytes<'a> = [u8; 16];
    fn fixed_width() -> Option<usize> { Some(16) }
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a> where Self: 'a {
        let arr: [u8; 16] = data.try_into().unwrap();
        RecordId(arr)
    }
    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a> {
        value.as_bytes().clone()
    }
    fn type_name() -> TypeName { TypeName::new("RecordId") }
}

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
        Ok(Self {
            db: Arc::new(db),
        })
    }
}

#[async_trait]
impl Repo for RedbRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let name = name.as_ref().to_string();
        let db = self.db.clone();

        Ok(Arc::new(RedbStore {
            db,
            table_name: name,
        }))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let name = name.as_ref().to_string();
        let db = self.db.clone();

        task::spawn_blocking(move || -> Result<bool, DbError> {
            let write_txn = db.begin_write()?;
            let def = TableDefinition::<RecordId, StorableRepoRecord>::new(&name);
            let deleted = write_txn.delete_table(def)?;
            write_txn.commit()?;
            Ok(deleted)
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))? // Здесь был лишний ?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let db = self.db.clone();

        task::spawn_blocking(move || -> Result<Vec<String>, DbError> {
            let read_txn = db.begin_read()?;
            let tables: Vec<String> = read_txn
                .list_tables()?
                .map(|t| t.name().to_string())
                .collect();
            Ok(tables)
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))? // Здесь был лишний ?
    }
}

// ============================================================================
// RedbStore - individual store implementation
// ============================================================================

pub struct RedbStore {
    db: Arc<Database>,
    table_name: String,
}

unsafe impl Send for RedbStore {}
unsafe impl Sync for RedbStore {}

#[async_trait]
impl Store for RedbStore {
    async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let value = value.clone();

        task::spawn_blocking(move || -> Result<RecordId, DbError> {
            let id = RecordId::new();
            let now = Utc::now().timestamp_micros() as u64;
            let record = (id, now, now, value);
            let storable = StorableRepoRecord(record);

            let write_txn = db.begin_write()?;
            {
                let table_def = TableDefinition::<RecordId, StorableRepoRecord>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;

                if table.get(id)?.is_some() {
                    return Err(DbError::Internal(format!("Key already exists: {:?}", id)));
                }
                table.insert(id, storable)?;
            }
            write_txn.commit()?;
            Ok(id)
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))? // Здесь был лишний ?
    }

    async fn set(&self, key: RecordId, value: &InnerValue) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let value = value.clone();

        task::spawn_blocking(move || -> Result<bool, DbError> {
            let write_txn = db.begin_write()?;
            {
                let table_def = TableDefinition::<RecordId, StorableRepoRecord>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;

                let created_at = if let Some(existing) = table.get(key)? {
                    existing.value().0.1
                } else {
                    Utc::now().timestamp_micros() as u64
                };

                let record = (key, created_at, Utc::now().timestamp_micros() as u64, value);
                table.insert(key, StorableRepoRecord(record))?;
            }
            write_txn.commit()?;
            Ok(true)
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))? // Здесь был лишний ?
    }

    async fn get(&self, key: RecordId) -> DbResult<RepoRecord> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        task::spawn_blocking(move || -> Result<RepoRecord, DbError> {
            let read_txn = db.begin_read()?;
            let table_def = TableDefinition::<RecordId, StorableRepoRecord>::new(&table_name);
            let table = read_txn.open_table(table_def)?;

            match table.get(key)? {
                Some(guard) => Ok(guard.value().0),
                None => Err(DbError::Internal(format!("Key not found: {:?}", key))),
            }
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))? // Здесь был лишний ?
    }

    async fn remove(&self, key: RecordId) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        task::spawn_blocking(move || -> Result<bool, DbError> {
            let write_txn = db.begin_write()?;
            let removed;
            {
                let table_def = TableDefinition::<RecordId, StorableRepoRecord>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;
                removed = table.remove(key)?.is_some();
            }
            write_txn.commit()?;
            Ok(removed)
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))? // Здесь был лишний ?
    }

    async fn iter(&self) -> DbResult<Vec<RepoRecord>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        task::spawn_blocking(move || -> Result<Vec<RepoRecord>, DbError> {
            let read_txn = db.begin_read()?;
            let table_def = TableDefinition::<RecordId, StorableRepoRecord>::new(&table_name);
            let table = read_txn.open_table(table_def)?;

            let mut result = Vec::new();
            for item in table.iter()? {
                let (_, val) = item?;
                result.push(val.value().0);
            }
            Ok(result)
        })
            .await
            .map_err(|e| DbError::Internal(e.to_string()))? // Здесь был лишний ?
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tokio::time::{sleep, Duration};

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
    async fn test_redb_repo_list_stores() {
        let path = "./test_data/redb_repo_list/db.redb";
        if let Some(parent) = Path::new(path).parent() {
            if parent.exists() {
                fs::remove_dir_all(parent).unwrap();
            }
        }

        let repo = RedbRepo::new(path).unwrap();

        let s1 = repo.store_get("table1").await.unwrap();
        s1.insert(&InnerValue::Int(1)).await.unwrap();

        let s2 = repo.store_get("table2").await.unwrap();
        s2.insert(&InnerValue::Int(2)).await.unwrap();

        let s3 = repo.store_get("table3").await.unwrap();
        s3.insert(&InnerValue::Int(3)).await.unwrap();

        let tables = repo.stores_list().await.unwrap();
        assert_eq!(tables.len(), 3);
        assert!(tables.contains(&"table1".to_string()));
        assert!(tables.contains(&"table2".to_string()));
        assert!(tables.contains(&"table3".to_string()));

        assert!(repo.store_delete("table2").await.unwrap());
        let tables = repo.stores_list().await.unwrap();
        assert_eq!(tables.len(), 2);
        assert!(!tables.contains(&"table2".to_string()));

        assert!(repo.store_delete("table1").await.unwrap());
        assert!(repo.store_delete("table3").await.unwrap());

        let s_check = repo.store_get("check_table").await.unwrap();
        s_check.insert(&InnerValue::Bool(true)).await.unwrap();

        let tables = repo.stores_list().await.unwrap();
        assert_eq!(tables.len(), 1);
        assert!(tables.contains(&"check_table".to_string()));
    }

    #[tokio::test]
    async fn test_redb_repo_store_isolation() {
        let path = "./test_data/redb_repo_isolation/db.redb";
        if let Some(parent) = Path::new(path).parent() {
            if parent.exists() {
                fs::remove_dir_all(parent).unwrap();
            }
        }

        let repo = RedbRepo::new(path).unwrap();

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

        assert!(store2.get(id1).await.is_err());
        assert!(store1.get(id2).await.is_err());
    }
}