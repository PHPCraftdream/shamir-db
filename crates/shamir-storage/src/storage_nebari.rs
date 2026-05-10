use super::types::{RecordKey, Repo, Store};
use crate::error::{DbError, DbResult};
use shamir_types::types::record_id::RecordId;
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use nebari::{
    io::fs::StdFile,
    tree::{Root, ScanEvaluation, Unversioned, UnversionedTreeRoot},
    Config, Roots, Tree,
};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task::spawn_blocking;

const META_TREE_NAME: &str = "__SHAMIR_META_STORES__";

// ============================================================================
// NebariRepo - manages multiple stores (trees)
// ============================================================================

#[derive(Clone)]
pub struct NebariRepo {
    roots: Arc<Roots<StdFile>>,
}

impl NebariRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let roots = Config::default_for(path.as_ref())
            .open()
            .map_err(|e| DbError::Storage(format!("NebariDB open: {}", e)))?;
        Ok(Self {
            roots: Arc::new(roots),
        })
    }
}

#[async_trait]
impl Repo for NebariRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let roots = self.roots.clone();
        let table_name = name.as_ref().to_string();

        // Регистрируем в мета-дереве
        let roots_clone = roots.clone();
        let table_name_clone = table_name.clone();
        spawn_blocking(move || -> DbResult<()> {
            let meta_tree = roots_clone
                .tree(Unversioned::tree(META_TREE_NAME))
                .map_err(|e| DbError::Storage(format!("NebariDB meta_tree: {}", e)))?;
            meta_tree
                .set(table_name_clone, [])
                .map_err(|e| DbError::Storage(format!("NebariDB meta_set: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))??;

        // Открываем дерево пользователя
        let tree = spawn_blocking(
            move || -> DbResult<Tree<UnversionedTreeRoot<()>, StdFile>> {
                roots
                    .tree(Unversioned::tree(table_name))
                    .map_err(|e| DbError::Storage(format!("NebariDB open_tree: {}", e)))
            },
        )
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))??;

        let store = NebariStore {
            tree: Arc::new(tree),
        };
        Ok(Arc::new(store))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let roots = self.roots.clone();
        let table_name = name.as_ref().to_string();

        spawn_blocking(move || -> DbResult<bool> {
            // Удаляем все записи из пользовательского дерева
            let user_tree = roots
                .tree(Unversioned::tree(table_name.clone()))
                .map_err(|e| DbError::Storage(e.to_string()))?;

            // Собираем все ключи
            let mut keys: Vec<Vec<u8>> = Vec::new();
            user_tree
                .scan::<nebari::Error, _, _, _, _>(
                    &(..),
                    true, // forward
                    |_, _, _| ScanEvaluation::ReadData,
                    |_, _| ScanEvaluation::ReadData,
                    |key, _, _| {
                        keys.push(key.to_vec());
                        Ok(())
                    },
                )
                .map_err(|e| DbError::Storage(format!("NebariDB scan: {}", e)))?;

            // Удаляем каждый ключ
            for key in keys {
                user_tree
                    .remove(&key)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
            }

            // Удаляем запись из мета-дерева
            let meta_tree = roots
                .tree(Unversioned::tree(META_TREE_NAME))
                .map_err(|e| DbError::Storage(e.to_string()))?;

            let existed = meta_tree
                .remove(table_name.as_bytes())
                .map_err(|e| DbError::Storage(e.to_string()))?
                .is_some();

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let roots = self.roots.clone();
        spawn_blocking(move || -> DbResult<Vec<String>> {
            let meta_tree = roots
                .tree(Unversioned::tree(META_TREE_NAME))
                .map_err(|e| DbError::Storage(format!("NebariDB meta_tree: {}", e)))?;

            let mut names = Vec::new();
            meta_tree
                .scan::<nebari::Error, _, _, _, _>(
                    &(..),
                    true, // forward
                    |_, _, _| ScanEvaluation::ReadData,
                    |_, _| ScanEvaluation::ReadData,
                    |key, _, _| {
                        let key_bytes = key.to_vec();
                        let name = String::from_utf8(key_bytes).map_err(|e| {
                            nebari::Error::from(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                e.to_string(),
                            ))
                        })?;
                        names.push(name);
                        Ok(())
                    },
                )
                .map_err(|e| DbError::Storage(format!("NebariDB scan: {}", e)))?;

            Ok(names)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }
}

// ============================================================================
// NebariStore - individual store (tree)
// ============================================================================

pub struct NebariStore {
    tree: Arc<Tree<UnversionedTreeRoot<()>, StdFile>>,
}

unsafe impl Send for NebariStore {}
unsafe impl Sync for NebariStore {}

#[async_trait]
impl Store for NebariStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<RecordKey> {
            let id = RecordId::new();
            let key = RecordKey::copy_from_slice(id.as_bytes());

            tree.set(key.to_vec(), value.to_vec())
                .map_err(|e| DbError::Storage(format!("NebariDB set: {}", e)))?;

            Ok(key)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let key_bytes = key.to_vec();

            let existed = tree
                .get(&key_bytes)
                .map_err(|e| DbError::Storage(format!("NebariDB get: {}", e)))?
                .is_some();

            tree.set(key_bytes, value.to_vec())
                .map_err(|e| DbError::Storage(format!("NebariDB set: {}", e)))?;

            Ok(!existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<Bytes> {
            let key_bytes = key.to_vec();
            let val = tree
                .get(&key_bytes)
                .map_err(|e| DbError::Storage(format!("NebariDB get: {}", e)))?
                .ok_or_else(|| DbError::NotFound(format!("{:?}", key)))?;

            Ok(Bytes::copy_from_slice(val.as_ref()))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let key_bytes = key.to_vec();
            let existed = tree
                .remove(&key_bytes)
                .map_err(|e| DbError::Storage(format!("NebariDB remove: {}", e)))?
                .is_some();

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let tree = self.tree.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            loop {
                let tree_clone = tree.clone();
                let start_key = last_key;

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = spawn_blocking(move || {
                    let mut out = Vec::new();
                    let mut next_cursor = None;
                    let mut count = 0;
                    let mut skipping = start_key.is_some();

                    tree_clone.scan::<nebari::Error, _, _, _, _>(
                        &(..),
                        true, // forward
                        |_, _, _| ScanEvaluation::ReadData,
                        |_, _| ScanEvaluation::ReadData,
                        |key, _, value| {
                            // Skip records until we pass start_key
                            if skipping {
                                if let Some(ref start) = start_key {
                                    if key.as_ref() == start.as_slice() {
                                        skipping = false; // Next record will be included
                                    }
                                } else {
                                    skipping = false;
                                }
                                return Ok(());
                            }

                            if count < batch_size {
                                next_cursor = Some(key.to_vec());
                                out.push((Bytes::copy_from_slice(key.as_ref()), Bytes::copy_from_slice(value.as_ref())));
                                count += 1;
                            }
                            Ok(())
                        },
                    )
                        .map_err(|e| DbError::Storage(format!("NebariDB scan: {}", e)))?;

                    Ok((out, next_cursor))
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

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let tree = self.tree.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            // Calculate upper bound for prefix
            let mut prefix_end = prefix.to_vec();
            if let Some(last_byte) = prefix_end.last_mut() {
                *last_byte = last_byte.wrapping_add(1);
            }

            loop {
                let tree_clone = tree.clone();
                let start_key = last_key;
                let prefix_clone = prefix.clone();
                let prefix_end_clone = prefix_end.clone();

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = spawn_blocking(move || {
                    let mut out = Vec::new();
                    let mut next_cursor = None;
                    let mut count = 0;
                    let mut skipping = start_key.is_some();

                    let range = if let Some(ref start) = start_key {
                        &start[..]..&prefix_end_clone
                    } else {
                        &prefix_clone[..]..&prefix_end_clone
                    };

                    tree_clone.scan::<nebari::Error, _, _, _, _>(
                        &range,
                        true,
                        |_, _, _| ScanEvaluation::ReadData,
                        |_, _| ScanEvaluation::ReadData,
                        |key, _, value| {
                            // Skip records until we pass start_key
                            if skipping {
                                if let Some(ref start) = start_key {
                                    if key.as_ref() == start.as_slice() {
                                        skipping = false;
                                    }
                                } else {
                                    skipping = false;
                                }
                                return Ok(());
                            }

                            // Stop if we've collected enough
                            if count >= batch_size {
                                return Ok(());
                            }

                            next_cursor = Some(key.to_vec());
                            out.push((Bytes::copy_from_slice(key.as_ref()), Bytes::copy_from_slice(value.as_ref())));
                            count += 1;
                            Ok(())
                        },
                    )
                    .map_err(|e| DbError::Storage(format!("NebariDB scan: {}", e)))?;

                    Ok((out, next_cursor))
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
    async fn test_nebari_repo_basic() {
        let path = "./test_data/nebari_repo_basic.nebari";
        if Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = NebariRepo::new(path).unwrap();
        let store = repo.store_get("test_table").await.unwrap();

        run_store_tests(store).await;

        assert!(repo.store_delete("test_table").await.unwrap());
    }

    #[tokio::test]
    async fn test_nebari_repo_list_stores() {
        let path = "./test_data/nebari_repo_list.nebari";
        if Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = NebariRepo::new(path).unwrap();

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
    async fn test_nebari_repo_store_isolation() {
        let path = "./test_data/nebari_repo_isolation.nebari";
        if Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = NebariRepo::new(path).unwrap();

        let store1 = repo.store_get("isolated_table1").await.unwrap();
        let store2 = repo.store_get("isolated_table2").await.unwrap();

        // Insert into table1
        let value1 = InnerValue::Str("table1_value".to_string());
        let key1 = store1.insert(value1.to_bytes()).await.unwrap();

        // Insert into table2
        let value2 = InnerValue::Str("table2_value".to_string());
        let key2 = store2.insert(value2.to_bytes()).await.unwrap();

        // Verify isolation - each table should have only 1 record
        assert_eq!(
            collect_stream(store1.iter_stream(1000))
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            collect_stream(store2.iter_stream(1000))
                .await
                .unwrap()
                .len(),
            1
        );

        // Verify correct values
        let retrieved_bytes1 = store1.get(key1.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes1).unwrap(), value1);

        let retrieved_bytes2 = store2.get(key2.clone()).await.unwrap();
        assert_eq!(InnerValue::from_bytes(retrieved_bytes2).unwrap(), value2);

        // Verify cross-table isolation (get should fail with NotFound)
        assert!(matches!(store2.get(key1).await, Err(DbError::NotFound(_))));
        assert!(matches!(store1.get(key2).await, Err(DbError::NotFound(_))));

        // Clean up
        repo.store_delete("isolated_table1").await.unwrap();
        repo.store_delete("isolated_table2").await.unwrap();
    }
}
