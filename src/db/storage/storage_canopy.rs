use super::types::{RecordKey, Repo, Store};
use crate::db::{DbError, DbResult};
use crate::types::record_id::RecordId;
use async_trait::async_trait;
use async_stream::stream;
use bytes::Bytes;
use canopydb::Database;
use futures::stream::Stream;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task::spawn_blocking;

const META_TREE_NAME: &[u8] = b"__SHAMIR_META_STORES__";

// ============================================================================
// CanopyRepo - manages multiple stores (trees)
// ============================================================================

#[derive(Clone)]
pub struct CanopyRepo {
    db: Arc<Database>,
}

impl CanopyRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let db =
            Database::new(path).map_err(|e| DbError::Storage(format!("CanopyDB new: {}", e)))?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl Repo for CanopyRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();
        let table_name_bytes = table_name.as_bytes().to_vec();

        spawn_blocking(move || -> DbResult<()> {
            let tx = db
                .begin_write()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_write: {}", e)))?;
            {
                let mut meta_tree = tx
                    .get_or_create_tree(META_TREE_NAME)
                    .map_err(|e| DbError::Storage(format!("CanopyDB get_meta_tree: {}", e)))?;
                meta_tree
                    .insert(&table_name_bytes, &[])
                    .map_err(|e| DbError::Storage(format!("CanopyDB meta_insert: {}", e)))?;
            }
            tx.commit()
                .map_err(|e| DbError::Storage(format!("CanopyDB commit: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))??;

        let store = CanopyStore {
            db: self.db.clone(),
            table_name,
        };
        Ok(Arc::new(store))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();

        spawn_blocking(move || -> DbResult<bool> {
            let tx = db
                .begin_write()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_write: {}", e)))?;
            let table_name_bytes = table_name.as_bytes();
            let existed;

            {
                // Clear the user tree
                if let Some(mut user_tree) = tx
                    .get_tree(table_name_bytes)
                    .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?
                {
                    let keys: Vec<Vec<u8>> = user_tree
                        .iter()
                        .map_err(|e| DbError::Storage(format!("CanopyDB iter: {}", e)))?
                        .map(|res| res.map(|(k, _v)| k.to_vec()))
                        .collect::<Result<_, _>>()
                        .map_err(|e| DbError::Storage(format!("CanopyDB iter collect: {}", e)))?;

                    for key in keys {
                        user_tree
                            .delete(&key)
                            .map_err(|e| DbError::Storage(format!("CanopyDB delete: {}", e)))?;
                    }
                }

                // Remove from meta tree
                let mut meta_tree = tx
                    .get_or_create_tree(META_TREE_NAME)
                    .map_err(|e| DbError::Storage(format!("CanopyDB get_meta_tree: {}", e)))?;

                existed = meta_tree
                    .delete(table_name_bytes)
                    .map_err(|e| DbError::Storage(format!("CanopyDB meta_delete: {}", e)))?;
            }

            tx.commit()
                .map_err(|e| DbError::Storage(format!("CanopyDB commit: {}", e)))?;

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let db = self.db.clone();
        spawn_blocking(move || -> DbResult<Vec<String>> {
            let tx = db
                .begin_read()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_read: {}", e)))?;

            let mut names = Vec::new();
            if let Some(meta_tree) = tx
                .get_tree(META_TREE_NAME)
                .map_err(|e| DbError::Storage(format!("CanopyDB get_meta_tree: {}", e)))?
            {
                for item in meta_tree
                    .iter()
                    .map_err(|e| DbError::Storage(format!("CanopyDB meta_iter: {}", e)))?
                {
                    let (key, _) =
                        item.map_err(|e| DbError::Storage(format!("CanopyDB iter item: {}", e)))?;
                    let name = String::from_utf8(key.to_vec())
                        .map_err(|e| DbError::Codec(format!("UTF-8 decode error: {}", e)))?;
                    names.push(name);
                }
            }
            Ok(names)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }
}

// ============================================================================
// CanopyStore - individual store (tree)
// ============================================================================

pub struct CanopyStore {
    db: Arc<Database>,
    table_name: String,
}

unsafe impl Send for CanopyStore {}
unsafe impl Sync for CanopyStore {}

#[async_trait]
impl Store for CanopyStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        spawn_blocking(move || -> DbResult<RecordKey> {
            let tx = db
                .begin_write()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_write: {}", e)))?;

            let id = RecordId::new();
            let key = RecordKey::copy_from_slice(id.as_bytes());

            {
                let mut tree = tx
                    .get_or_create_tree(table_name.as_bytes())
                    .map_err(|e| {
                        DbError::Storage(format!("CanopyDB get_or_create_tree: {}", e))
                    })?;

                tree.insert(&key[..], &*value)
                    .map_err(|e| DbError::Storage(format!("CanopyDB insert: {}", e)))?;
            }

            tx.commit()
                .map_err(|e| DbError::Storage(format!("CanopyDB commit: {}", e)))?;

            Ok(key)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let tx = db
                .begin_write()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_write: {}", e)))?;

            let created;
            {
                let mut tree = tx
                    .get_or_create_tree(table_name.as_bytes())
                    .map_err(|e| {
                        DbError::Storage(format!("CanopyDB get_or_create_tree: {}", e))
                    })?;

                let existed = tree
                    .get(&key[..])
                    .map_err(|e| DbError::Storage(format!("CanopyDB get: {}", e)))?
                    .is_some();

                tree.insert(&key[..], &*value)
                    .map_err(|e| DbError::Storage(format!("CanopyDB insert: {}", e)))?;

                created = !existed;
            }

            tx.commit()
                .map_err(|e| DbError::Storage(format!("CanopyDB commit: {}", e)))?;

            Ok(created)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        spawn_blocking(move || -> DbResult<Bytes> {
            let tx = db
                .begin_read()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_read: {}", e)))?;

            let tree = tx
                .get_tree(table_name.as_bytes())
                .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?
                .ok_or_else(|| DbError::NotFound(table_name.clone()))?;

            let val = tree
                .get(&key[..])
                .map_err(|e| DbError::Storage(format!("CanopyDB get: {}", e)))?
                .ok_or_else(|| DbError::NotFound(format!("{:?}", key)))?;

            Ok(Bytes::copy_from_slice(&val))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let tx = db
                .begin_write()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_write: {}", e)))?;

            let existed;
            {
                let mut tree = tx
                    .get_tree(table_name.as_bytes())
                    .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?
                    .ok_or_else(|| DbError::NotFound(table_name.clone()))?;

                existed = tree
                    .delete(&key[..])
                    .map_err(|e| DbError::Storage(format!("CanopyDB delete: {}", e)))?;
            }

            tx.commit()
                .map_err(|e| DbError::Storage(format!("CanopyDB commit: {}", e)))?;

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn iter(&self) -> DbResult<Vec<(RecordKey, Bytes)>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        spawn_blocking(move || -> DbResult<Vec<(RecordKey, Bytes)>> {
            let tx = db
                .begin_read()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_read: {}", e)))?;

            let tree_res = tx
                .get_tree(table_name.as_bytes())
                .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?;

            if let Some(tree) = tree_res {
                let mut out = Vec::new();
                for item in tree
                    .iter()
                    .map_err(|e| DbError::Storage(format!("CanopyDB iter: {}", e)))?
                {
                    let (key, val) =
                        item.map_err(|e| DbError::Storage(format!("CanopyDB iter item: {}", e)))?;
                    out.push((Bytes::copy_from_slice(&key), Bytes::copy_from_slice(&val)));
                }
                Ok(out)
            } else {
                Ok(vec![])
            }
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    fn iter_stream(&self, batch_size: usize) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            loop {
                let db_clone = db.clone();
                let table_name_clone = table_name.clone();
                let start_key = last_key;

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = spawn_blocking(move || {
                    let tx = db_clone
                        .begin_read()
                        .map_err(|e| DbError::Storage(format!("CanopyDB begin_read: {}", e)))?;

                    let tree_res = tx
                        .get_tree(table_name_clone.as_bytes())
                        .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?;

                    if let Some(tree) = tree_res {
                        let mut out = Vec::new();
                        let mut next_cursor = None;

                        // Use range starting from cursor
                        let iter_result = if let Some(start) = start_key {
                            tree.range(start.as_slice()..)
                        } else {
                            tree.iter()
                        };

                        let mut iter = iter_result.map_err(|e| DbError::Storage(format!("CanopyDB iter: {}", e)))?;

                        // Collect up to batch_size items
                        for _ in 0..batch_size {
                            match iter.next() {
                                Some(Ok(item)) => {
                                    let (key, val) = item;
                                    next_cursor = Some(key.to_vec());
                                    out.push((Bytes::copy_from_slice(&key), Bytes::copy_from_slice(&val)));
                                }
                                Some(Err(e)) => {
                                    return Err(DbError::Storage(format!("CanopyDB iter item: {}", e)));
                                }
                                None => break, // No more items
                            }
                        }

                        Ok((out, next_cursor))
                    } else {
                        Ok((vec![], None))
                    }
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let (batch, next_key) = batch?;

                if batch.is_empty() {
                    break;
                }

                last_key = next_key;
                yield Ok(batch);
            }
        })
    }

    async fn scan_prefix(&self, prefix: Bytes) -> DbResult<Vec<(RecordKey, Bytes)>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        spawn_blocking(move || -> DbResult<Vec<(RecordKey, Bytes)>> {
            let tx = db
                .begin_read()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_read: {}", e)))?;

            let tree_res = tx
                .get_tree(table_name.as_bytes())
                .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?;

            if let Some(tree) = tree_res {
                let mut out = Vec::new();
                let prefix_slice = &prefix[..];

                // Use range with prefix bounds
                let mut prefix_end = prefix.to_vec();
                if let Some(last_byte) = prefix_end.last_mut() {
                    *last_byte = last_byte.wrapping_add(1);
                }

                let iter_result = tree.range(prefix_slice..&prefix_end);
                let mut iter = iter_result
                    .map_err(|e| DbError::Storage(format!("CanopyDB iter: {}", e)))?;

                for item in &mut iter {
                    let (key, val) =
                        item.map_err(|e| DbError::Storage(format!("CanopyDB iter item: {}", e)))?;
                    out.push((Bytes::copy_from_slice(&key), Bytes::copy_from_slice(&val)));
                }

                Ok(out)
            } else {
                Ok(vec![])
            }
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
                let start_key = last_key;
                let prefix_clone = prefix.clone();
                let prefix_end_clone = prefix_end.clone();

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = spawn_blocking(move || {
                    let tx = db_clone
                        .begin_read()
                        .map_err(|e| DbError::Storage(format!("CanopyDB begin_read: {}", e)))?;

                    let tree_res = tx
                        .get_tree(table_name_clone.as_bytes())
                        .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?;

                    if let Some(tree) = tree_res {
                        let mut out = Vec::new();
                        let mut next_cursor = None;

                        // Use range with exclusive start to avoid duplicates
                        use std::ops::Bound;

                        let range_bounds: (Bound<&[u8]>, Bound<&[u8]>) = if let Some(start) = &start_key {
                            (Bound::Excluded(start.as_slice()), Bound::Excluded(prefix_end_clone.as_slice()))
                        } else {
                            (Bound::Included(prefix_clone.as_ref()), Bound::Excluded(prefix_end_clone.as_slice()))
                        };

                        let mut iter = tree
                            .range::<&[u8]>(range_bounds)
                            .map_err(|e| DbError::Storage(format!("CanopyDB iter: {}", e)))?;

                        // Collect up to batch_size items
                        for _ in 0..batch_size {
                            match iter.next() {
                                Some(Ok(item)) => {
                                    let (key, val) = item;
                                    // Stop if no longer starts with prefix
                                    if !key.starts_with(&prefix_clone) {
                                        break;
                                    }
                                    next_cursor = Some(key.to_vec());
                                    out.push((Bytes::copy_from_slice(&key), Bytes::copy_from_slice(&val)));
                                }
                                Some(Err(e)) => {
                                    return Err(DbError::Storage(format!("CanopyDB iter item: {}", e)));
                                }
                                None => break,
                            }
                        }

                        Ok((out, next_cursor))
                    } else {
                        Ok((vec![], None))
                    }
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

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
    use super::*;
    use crate::types::value::InnerValue;
    use futures::StreamExt;
    use std::fs;
    use tokio::time::{sleep, Duration};
    use crate::types::record_id::RecordId;

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
    async fn test_canopy_repo_basic() {
        let path = "./test_data/canopy_repo_basic";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }
        fs::create_dir_all(path).unwrap();

        let repo = CanopyRepo::new(path).unwrap();
        let store = repo.store_get("test_table").await.unwrap();

        run_store_tests(store).await;

        assert!(repo.store_delete("test_table").await.unwrap());
    }

    #[tokio::test]
    async fn test_canopy_repo_list_stores() {
        let path = "./test_data/canopy_repo_list";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }
        fs::create_dir_all(path).unwrap();

        let repo = CanopyRepo::new(path).unwrap();

        // Check empty list
        assert_eq!(repo.stores_list().await.unwrap().len(), 0);

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
    async fn test_canopy_repo_store_isolation() {
        let path = "./test_data/canopy_repo_isolation";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }
        fs::create_dir_all(path).unwrap();

        let repo = CanopyRepo::new(path).unwrap();

        let store1 = repo.store_get("isolated_table1").await.unwrap();
        let store2 = repo.store_get("isolated_table2").await.unwrap();

        // Insert into table1
        let value1 = InnerValue::Str("table1_value".to_string());
        let key1 = store1.insert(value1.to_bytes()).await.unwrap();

        // Insert into table2
        let value2 = InnerValue::Str("table2_value".to_string());
        let key2 = store2.insert(value2.to_bytes()).await.unwrap();

        // Verify isolation - each table should have only 1 record
        assert_eq!(store1.iter().await.unwrap().len(), 1);
        assert_eq!(store2.iter().await.unwrap().len(), 1);

        // Verify correct values
        let retrieved_bytes1 = store1.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes1).unwrap(), value1);

        let retrieved_bytes2 = store2.get(key2.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes2).unwrap(), value2);

        // Verify cross-table isolation (get should fail with NotFound because the tree itself won't be found)
        assert!(matches!(
            store2.get(key1).await,
            Err(DbError::NotFound(_))
        ));
        assert!(matches!(
            store1.get(key2).await,
            Err(DbError::NotFound(_))
        ));

        // Clean up
        repo.store_delete("isolated_table1").await.unwrap();
        repo.store_delete("isolated_table2").await.unwrap();
    }

    #[tokio::test]
    async fn test_canopy_prefix_scan() {
        let path = "./test_data/canopy_prefix_scan";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }
        fs::create_dir_all(path).unwrap();

        let repo = CanopyRepo::new(path).unwrap();
        let db = repo.db.clone();

        // Create CanopyStore directly to access PrefixScan
        let table_name = "test_table";
        let tx = db.begin_write().unwrap();
        {
            let _tree = tx.get_or_create_tree(table_name.as_bytes()).unwrap();
        }
        tx.commit().unwrap();

        let store = CanopyStore {
            db,
            table_name: table_name.to_string(),
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
