use super::types::{RecordKey, Repo, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition, TableHandle};
use shamir_types::types::record_id::RecordId;
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
    /// Open / create a redb-backed repo at `path`.
    ///
    /// Synchronous on purpose — `Database::create` does a blocking
    /// fsync chain (file open, allocate, write metadata). The only
    /// in-tree async caller is `RedbRepoFactory::create` which
    /// already wraps this in `tokio::task::spawn_blocking`. Tests
    /// call it directly from sync code. Do NOT call this from an
    /// `async fn` body without a `spawn_blocking` / `block_in_place`
    /// wrapper — it will stall the tokio worker (§B11).
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

            let mut write_txn = db.begin_write()?;
            // Default durability skips fsync; explicit `Store::flush()`
            // forces the sync point. Matches sled's amortised model.
            write_txn
                .set_durability(Durability::None)
                .map_err(|e| DbError::Storage(format!("Redb set_durability: {}", e)))?;
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
            let mut write_txn = db.begin_write()?;
            write_txn
                .set_durability(Durability::None)
                .map_err(|e| DbError::Storage(format!("Redb set_durability: {}", e)))?;
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
            let mut write_txn = db.begin_write()?;
            write_txn
                .set_durability(Durability::None)
                .map_err(|e| DbError::Storage(format!("Redb set_durability: {}", e)))?;
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

    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let start_bytes = start_inclusive.map(|b| b.to_vec());
        let end_bytes = end_inclusive.map(|b| b.to_vec());

        Box::pin(stream! {
            // Cursor advances past the last key we yielded.
            let mut cursor: Option<Vec<u8>> = None;

            loop {
                let db_clone = db.clone();
                let table_name_clone = table_name.clone();
                let cur = cursor.clone();
                let initial_start = start_bytes.clone();
                let upper = end_bytes.clone();

                let batch: DbResult<Vec<(Bytes, Bytes)>> = task::spawn_blocking(move || {
                    let read_txn = db_clone.begin_read()?;
                    let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name_clone);
                    let table = read_txn.open_table(table_def)?;

                    // After the first batch we resume past the
                    // previously-yielded key (Excluded). Otherwise we
                    // start at the user-supplied lower bound (Included)
                    // or Unbounded.
                    let lower = match (&cur, &initial_start) {
                        (Some(c), _) => Bound::Excluded(c.as_slice()),
                        (None, Some(s)) => Bound::Included(s.as_slice()),
                        (None, None) => Bound::Unbounded,
                    };
                    let upper = match &upper {
                        Some(e) => Bound::Included(e.as_slice()),
                        None => Bound::Unbounded,
                    };

                    let range: (Bound<&[u8]>, Bound<&[u8]>) = (lower, upper);
                    let mut items = Vec::new();
                    for item in table.range::<&[u8]>(range)?.take(batch_size) {
                        let (key, val) = item?;
                        items.push((
                            Bytes::copy_from_slice(key.value()),
                            Bytes::copy_from_slice(val.value()),
                        ));
                    }
                    Ok(items)
                })
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

                let batch = batch?;
                if batch.is_empty() {
                    break;
                }
                cursor = batch.last().map(|(k, _)| k.to_vec());
                yield Ok(batch);
            }
        })
    }

    /// Vectored read: ONE read transaction, ONE spawn_blocking,
    /// N `table.get` calls. Compared to N×`get` (each its own
    /// begin_read + open_table + spawn_blocking) this collapses N
    /// fixed-cost setups into one.
    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<Vec<Option<Bytes>>> {
            let read_txn = db.begin_read()?;
            let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name);
            let table = read_txn.open_table(table_def)?;
            let mut out = Vec::with_capacity(keys.len());
            for k in keys {
                match table.get(&k[..])? {
                    Some(guard) => out.push(Some(Bytes::copy_from_slice(guard.value()))),
                    None => out.push(None),
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    /// Reverse range scan via redb's `Table::range(...).rev()`.
    /// Cursor advances downward (upper-side shrinks each batch).
    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let start_bytes = start_inclusive.map(|b| b.to_vec());
        let end_bytes = end_inclusive.map(|b| b.to_vec());

        Box::pin(stream! {
            let mut cursor: Option<Vec<u8>> = None;

            loop {
                let db_clone = db.clone();
                let table_name_clone = table_name.clone();
                let cur = cursor.clone();
                let lower_init = start_bytes.clone();
                let upper_init = end_bytes.clone();

                let batch: DbResult<Vec<(Bytes, Bytes)>> = task::spawn_blocking(move || {
                    let read_txn = db_clone.begin_read()?;
                    let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name_clone);
                    let table = read_txn.open_table(table_def)?;

                    let lower = match &lower_init {
                        Some(s) => Bound::Included(s.as_slice()),
                        None => Bound::Unbounded,
                    };
                    let upper = match (&cur, &upper_init) {
                        (Some(c), _) => Bound::Excluded(c.as_slice()),
                        (None, Some(e)) => Bound::Included(e.as_slice()),
                        (None, None) => Bound::Unbounded,
                    };

                    let range: (Bound<&[u8]>, Bound<&[u8]>) = (lower, upper);
                    let mut items = Vec::new();
                    for item in table.range::<&[u8]>(range)?.rev().take(batch_size) {
                        let (key, val) = item?;
                        items.push((
                            Bytes::copy_from_slice(key.value()),
                            Bytes::copy_from_slice(val.value()),
                        ));
                    }
                    Ok(items)
                })
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

                let batch = batch?;
                if batch.is_empty() {
                    break;
                }
                cursor = batch.last().map(|(k, _)| k.to_vec());
                yield Ok(batch);
            }
        })
    }

    /// Batched insert via one WriteTransaction. Even with the
    /// per-write Durability::None path the txn-setup cost amortises
    /// down to ~zero per record when batched.
    async fn insert_many(&self, values: Vec<Bytes>) -> DbResult<Vec<RecordKey>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<Vec<RecordKey>> {
            let mut write_txn = db.begin_write()?;
            write_txn
                .set_durability(Durability::None)
                .map_err(|e| DbError::Storage(format!("Redb set_durability: {}", e)))?;
            let mut ids = Vec::with_capacity(values.len());
            {
                let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;
                for value in values {
                    let id = RecordId::new();
                    let key = RecordKey::copy_from_slice(id.as_bytes());
                    if table.get(&key[..])?.is_some() {
                        return Err(DbError::KeyExists(format!("Key already exists: {:?}", key)));
                    }
                    table.insert(&key[..], &value[..])?;
                    ids.push(key);
                }
            }
            write_txn.commit()?;
            Ok(ids)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<Vec<bool>> {
            let mut write_txn = db.begin_write()?;
            write_txn
                .set_durability(Durability::None)
                .map_err(|e| DbError::Storage(format!("Redb set_durability: {}", e)))?;
            let mut flags = Vec::with_capacity(items.len());
            {
                let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;
                for (key, value) in items {
                    let old = table.insert(&key[..], &value[..])?;
                    flags.push(old.is_none());
                }
            }
            write_txn.commit()?;
            Ok(flags)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<Vec<bool>> {
            let mut write_txn = db.begin_write()?;
            write_txn
                .set_durability(Durability::None)
                .map_err(|e| DbError::Storage(format!("Redb set_durability: {}", e)))?;
            let mut flags = Vec::with_capacity(keys.len());
            {
                let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;
                for key in keys {
                    flags.push(table.remove(&key[..])?.is_some());
                }
            }
            write_txn.commit()?;
            Ok(flags)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    /// Native atomic `transact` via redb `WriteTransaction`.
    ///
    /// Opens one write transaction, applies all `KvOp`s (set / remove)
    /// within the same table handle, then commits. If any operation
    /// fails, the transaction is dropped (implicitly aborted by redb)
    /// — no partial state is observable.
    async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        task::spawn_blocking(move || -> DbResult<()> {
            let mut write_txn = db.begin_write()?;
            write_txn
                .set_durability(Durability::None)
                .map_err(|e| DbError::Storage(format!("Redb set_durability: {}", e)))?;
            {
                let table_def = TableDefinition::<&[u8], &[u8]>::new(&table_name);
                let mut table = write_txn.open_table(table_def)?;
                for op in ops {
                    match op {
                        super::types::KvOp::Set(k, v) => {
                            table.insert(&k[..], &v[..])?;
                        }
                        super::types::KvOp::Remove(k) => {
                            table.remove(&k[..])?;
                        }
                    }
                }
            }
            write_txn.commit()?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    /// Explicit fsync. Per-write commits run with `Durability::None`
    /// (skips fsync, data goes to the OS page cache, visible to
    /// subsequent reads). `flush()` runs an empty commit with
    /// `Durability::Immediate`, forcing pending writes to disk.
    async fn flush(&self) -> DbResult<()> {
        let db = self.db.clone();
        task::spawn_blocking(move || -> DbResult<()> {
            let mut write_txn = db.begin_write()?;
            write_txn
                .set_durability(Durability::Immediate)
                .map_err(|e| DbError::Storage(format!("Redb set_durability: {}", e)))?;
            write_txn.commit()?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }
}

// ============================================================================
// Tests
// ============================================================================
