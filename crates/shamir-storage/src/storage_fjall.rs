use super::types::{RecordKey, Repo, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use futures::stream::Stream;
use shamir_types::types::record_id::RecordId;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task;

// ============================================================================
// FjallRepo - manages database connection
// ============================================================================

pub struct FjallRepo {
    db: Arc<Database>,
}

impl FjallRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let db = Database::builder(path.as_ref())
            .open()
            .map_err(|e| DbError::Storage(e.to_string()))?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl Repo for FjallRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();

        let keyspace = task::spawn_blocking(move || -> DbResult<Keyspace> {
            db.keyspace(&table_name, KeyspaceCreateOptions::default)
                .map_err(|e| DbError::Storage(e.to_string()))
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))??;

        Ok(Arc::new(FjallStore {
            keyspace,
            db: self.db.clone(),
        }))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();

        task::spawn_blocking(move || -> DbResult<bool> {
            let keyspace = db
                .keyspace(&table_name, KeyspaceCreateOptions::default)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            db.delete_keyspace(keyspace)
                .map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(true)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let db = self.db.clone();
        task::spawn_blocking(move || -> DbResult<Vec<String>> {
            let names: Vec<String> = db
                .list_keyspace_names()
                .into_iter()
                .map(|s| s.to_string())
                .collect();
            Ok(names)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }
}

// ============================================================================
// FjallStore - individual store (keyspace)
// ============================================================================

pub struct FjallStore {
    keyspace: Keyspace,
    /// Kept alongside the keyspace so `Store::flush()` can call
    /// `Database::persist(PersistMode::SyncAll)` — fjall journals
    /// to a per-database WAL, not per-keyspace, so durability is
    /// the database's concern.
    db: Arc<Database>,
}

#[async_trait]
impl Store for FjallStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let keyspace = self.keyspace.clone();

        task::spawn_blocking(move || -> DbResult<RecordKey> {
            let id = RecordId::new();
            let key = RecordKey::copy_from_slice(id.as_bytes());

            // §B13 (acknowledged, benign here): `contains_key` then
            // `insert` is two separate fjall ops — formally a TOCTOU
            // window. Safe in this codepath because `RecordId::new()`
            // returns a fresh random 128-bit id per call; the
            // collision probability across concurrent inserts is
            // negligible (~2⁻¹²⁸). fjall 3.0 `Keyspace::insert`
            // returns `Result<(), Error>` with no prior-value, and
            // there is no `compare_and_swap` at this layer — atomic
            // check-and-insert would require a transaction (extra
            // round-trip per write, regressing hot-path throughput).
            if keyspace
                .contains_key(&key[..])
                .map_err(|e| DbError::Storage(e.to_string()))?
            {
                return Err(DbError::KeyExists(format!("Key already exists: {:?}", key)));
            }

            keyspace
                .insert(&key[..], &*value)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            Ok(key)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<bool> {
            // §B13 (acknowledged): same TOCTOU shape as `insert`
            // above. The engine layer never issues two concurrent
            // `set` calls for the same `RecordKey` for a single
            // table (writes are serialised through `TableManager`
            // dispatch), so the `existed` flag stays consistent
            // with the actual write under normal use. Concurrent
            // calls from outside the engine (e.g. tooling) would
            // race; documented here so the contract is explicit.
            let existed = keyspace
                .contains_key(&key[..])
                .map_err(|e| DbError::Storage(e.to_string()))?;

            keyspace
                .insert(&key[..], &*value)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            Ok(!existed)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<Bytes> {
            match keyspace
                .get(&key[..])
                .map_err(|e| DbError::Storage(e.to_string()))?
            {
                Some(slice) => Ok(Bytes::copy_from_slice(&slice)),
                None => Err(DbError::NotFound(format!("record not found: {:?}", key))),
            }
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    /// Reverse range scan using fjall's `keyspace.range(...)` which
    /// implements `DoubleEndedIterator` — so `.rev()` walks the
    /// LSM tree backwards natively, no in-memory collect.
    /// Replaces the default `collect-forward + reverse` impl.
    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let keyspace = self.keyspace.clone();
        let start_bytes = start_inclusive.map(|b| b.to_vec());
        let end_bytes = end_inclusive.map(|b| b.to_vec());

        Box::pin(stream! {
            // Cursor walks downward; upper bound shrinks each batch.
            let mut cursor: Option<Vec<u8>> = None;

            loop {
                let keyspace_clone = keyspace.clone();
                let cur = cursor.clone();
                let lower_init = start_bytes.clone();
                let upper_init = end_bytes.clone();

                let batch: DbResult<Vec<(Bytes, Bytes)>> = task::spawn_blocking(move || {
                    use std::ops::Bound;
                    let lower: Bound<Vec<u8>> = match &lower_init {
                        Some(s) => Bound::Included(s.clone()),
                        None => Bound::Unbounded,
                    };
                    let upper: Bound<Vec<u8>> = match (&cur, &upper_init) {
                        (Some(c), _) => Bound::Excluded(c.clone()),
                        (None, Some(e)) => Bound::Included(e.clone()),
                        (None, None) => Bound::Unbounded,
                    };

                    let mut items = Vec::new();
                    for guard in keyspace_clone.range((lower, upper)).rev().take(batch_size) {
                        let (key, val) = guard
                            .into_inner()
                            .map_err(|e| DbError::Storage(e.to_string()))?;
                        items.push((
                            Bytes::copy_from_slice(&key),
                            Bytes::copy_from_slice(&val),
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

    /// Native atomic `transact` via fjall `OwnedWriteBatch`.
    ///
    /// `Database::batch()` returns an `OwnedWriteBatch` that collects
    /// insert/remove ops across keyspaces. `commit()` applies them
    /// atomically — all succeed or none are visible.
    async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let db = self.db.clone();
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<()> {
            let mut batch = db.batch();
            for op in ops {
                match op {
                    super::types::KvOp::Set(k, v) => {
                        batch.insert(&keyspace, k.as_ref(), v.as_ref());
                    }
                    super::types::KvOp::Remove(k) => {
                        batch.remove(&keyspace, k.as_ref());
                    }
                }
            }
            batch
                .commit()
                .map_err(|e| DbError::Storage(format!("Fjall batch commit: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    /// Force the WAL to fsync-on-disk. fjall buffers individual
    /// writes in the journal; `persist(SyncAll)` fsyncs the journal
    /// + writes any pending metadata. Reachable through
    /// `Arc<dyn Store>` — without this override callers hitting
    /// the default no-op would silently get "eventually durable"
    /// even after an explicit `flush()`.
    async fn flush(&self) -> DbResult<()> {
        let db = self.db.clone();
        task::spawn_blocking(move || -> DbResult<()> {
            db.persist(PersistMode::SyncAll)
                .map_err(|e| DbError::Storage(format!("Fjall persist: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<Vec<Option<Bytes>>> {
            let mut out = Vec::with_capacity(keys.len());
            for k in keys {
                match keyspace
                    .get(&k[..])
                    .map_err(|e| DbError::Storage(e.to_string()))?
                {
                    Some(slice) => out.push(Some(Bytes::copy_from_slice(&slice))),
                    None => out.push(None),
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<bool> {
            // Check if key exists
            let existed = keyspace
                .contains_key(&key[..])
                .map_err(|e| DbError::Storage(e.to_string()))?;

            if existed {
                keyspace
                    .remove(&key[..])
                    .map_err(|e| DbError::Storage(e.to_string()))?;
            }

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let keyspace = self.keyspace.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            loop {
                let keyspace_clone = keyspace.clone();
                let start_key = last_key;

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = task::spawn_blocking(move || {
                    let mut items = Vec::new();
                    let mut last_batch_key: Option<Vec<u8>> = None;

                    let mut iter = keyspace_clone.iter();

                    // If cursor specified, skip until we pass it
                    if let Some(start) = start_key {
                        for guard in iter.by_ref() {
                            let (key, _) = guard
                                .into_inner()
                                .map_err(|e| DbError::Storage(e.to_string()))?;
                            if key.as_ref() == start.as_slice() {
                                // Found it, next item will be first in batch
                                break;
                            }
                        }
                    }

                    for guard in iter.take(batch_size) {
                        let (key, value_slice) = guard
                            .into_inner()
                            .map_err(|e| DbError::Storage(e.to_string()))?;

                        last_batch_key = Some(key.to_vec());
                        items.push((Bytes::copy_from_slice(&key), Bytes::copy_from_slice(&value_slice)));
                    }

                    Ok((items, last_batch_key))
                })
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

                let (batch, next_key) = batch?;

                if batch.is_empty() {
                    break;
                }

                last_key = next_key;
                yield Ok(batch);
            }
        })
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let keyspace = self.keyspace.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;
            let prefix_slice = prefix.to_vec();

            loop {
                let keyspace_clone = keyspace.clone();
                let start_key = last_key;
                let prefix_clone = prefix_slice.clone();

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = task::spawn_blocking(move || {
                    // First pass: collect all matching records (fjall iter order is not guaranteed)
                    let mut all_matches: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                    for guard in keyspace_clone.iter() {
                        let (key, value) = guard
                            .into_inner()
                            .map_err(|e| DbError::Storage(e.to_string()))?;

                        if key.starts_with(&prefix_clone) {
                            all_matches.push((key.to_vec(), value.to_vec()));
                        }
                    }

                    // Filter by cursor and take batch_size
                    let start_idx = if let Some(start) = &start_key {
                        all_matches.iter().position(|(k, _)| k.as_slice() > start.as_slice()).unwrap_or(all_matches.len())
                    } else {
                        0
                    };

                    let batch_range = start_idx..(start_idx + batch_size).min(all_matches.len());
                    let batch_end = batch_range.end;
                    let items: Vec<_> = all_matches[batch_range]
                        .iter()
                        .map(|(k, v)| (Bytes::copy_from_slice(k), Bytes::copy_from_slice(v)))
                        .collect();

                    let last_key = batch_end.checked_sub(1).and_then(|i| all_matches.get(i).map(|(k, _)| k.clone()));

                    Ok((items, last_key))
                })
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

                let (batch, next_key) = batch?;

                if batch.is_empty() {
                    break;
                }

                last_key = next_key;
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
    use shamir_types::types::record_id::RecordId;
    use shamir_types::types::value::InnerValue;
    use std::fs;
    use tokio::time::{sleep, Duration};

    async fn run_store_tests(store: Arc<dyn Store>) {
        // Test insert and get
        let value1 = InnerValue::Str("hello".to_string());
        let key1 = store.insert(value1.to_bytes().unwrap()).await.unwrap();
        let retrieved_bytes = store.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes).unwrap(), value1);

        // Test set (update)
        sleep(Duration::from_micros(50)).await;
        let value2 = InnerValue::Str("world".to_string());
        let created = store
            .set(key1.clone(), value2.to_bytes().unwrap())
            .await
            .unwrap();
        assert!(!created); // Should be false, as it's an update
        let retrieved_bytes2 = store.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes2).unwrap(), value2);

        // Test set (create)
        let id2 = RecordId::new();
        let key2 = Bytes::copy_from_slice(id2.as_bytes());
        let value3 = InnerValue::Int(123);
        let created2 = store
            .set(key2.clone(), value3.to_bytes().unwrap())
            .await
            .unwrap();
        assert!(created2); // Should be true, as it's a new record
        let retrieved_bytes3 = store.get(key2.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes3).unwrap(), value3);

        // Test iter
        let value4 = InnerValue::Bool(true);
        let _key3 = store.insert(value4.to_bytes().unwrap()).await.unwrap();
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
    async fn test_fjall_repo_basic() {
        let path = "./test_data/fjall_repo_basic";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = FjallRepo::new(path).unwrap();
        let store = repo.store_get("test_table").await.unwrap();

        run_store_tests(store).await;

        assert!(repo.store_delete("test_table").await.unwrap());
    }

    #[tokio::test]
    async fn test_fjall_batch_ops() {
        let path = "./test_data/fjall_batch_ops";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }
        let repo = FjallRepo::new(path).unwrap();
        let store = repo.store_get("batch").await.unwrap();
        super::super::types::run_batch_store_tests(store).await;
    }

    /// Fjall transact test -- verifies all ops applied atomically via
    /// one `OwnedWriteBatch::commit`.
    ///
    /// fjall's `Keyspace::get()` reads from the current LSM state
    /// without snapshot isolation across multiple calls, so we only
    /// verify final state here (write atomicity).
    #[tokio::test]
    async fn test_fjall_transact_atomic() {
        use super::super::types::KvOp;

        let path = "./test_data/fjall_transact";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }
        let repo = FjallRepo::new(path).unwrap();
        let store = repo.store_get("transact_test").await.unwrap();

        // Seed
        let k1: RecordKey = Bytes::from_static(b"k1");
        let k2: RecordKey = Bytes::from_static(b"k2");
        let k3: RecordKey = Bytes::from_static(b"k3");
        store
            .set(k1.clone(), Bytes::from_static(b"old1"))
            .await
            .unwrap();
        store
            .set(k2.clone(), Bytes::from_static(b"old2"))
            .await
            .unwrap();
        store
            .set(k3.clone(), Bytes::from_static(b"to_remove"))
            .await
            .unwrap();

        // Mixed transact: update k1, update k2, remove k3
        store
            .transact(vec![
                KvOp::Set(k1.clone(), Bytes::from_static(b"new1")),
                KvOp::Set(k2.clone(), Bytes::from_static(b"new2")),
                KvOp::Remove(k3.clone()),
            ])
            .await
            .unwrap();

        assert_eq!(store.get(k1).await.unwrap().as_ref(), b"new1");
        assert_eq!(store.get(k2).await.unwrap().as_ref(), b"new2");
        assert!(store.get(k3).await.is_err(), "k3 should be removed");

        fs::remove_dir_all(path).ok();
    }

    #[tokio::test]
    async fn test_fjall_repo_list_and_delete_stores() {
        let path = "./test_data/fjall_repo_list";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = FjallRepo::new(path).unwrap();

        let _ = repo.store_get("table1").await.unwrap();
        let _ = repo.store_get("table2").await.unwrap();
        let _ = repo.store_get("table3").await.unwrap();

        let mut tables = repo.stores_list().await.unwrap();
        tables.sort(); // Order is not guaranteed
        assert_eq!(tables, vec!["table1", "table2", "table3"]);

        assert!(repo.store_delete("table2").await.unwrap());

        let mut tables_after_delete = repo.stores_list().await.unwrap();
        tables_after_delete.sort();
        assert_eq!(tables_after_delete, vec!["table1", "table3"]);
    }
}
