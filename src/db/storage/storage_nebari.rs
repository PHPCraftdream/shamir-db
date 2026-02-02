use super::types::{Repo, Store};
use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use crate::types::repo_record::RepoRecord;
use crate::types::value::InnerValue;
use async_trait::async_trait;
use chrono::Utc;
use nebari::{
    tree::{Root, Unversioned, UnversionedTreeRoot, ScanEvaluation},
    Config, Roots, Tree,
    io::fs::StdFile,
};
use std::path::Path;
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
                .set(table_name_clone, &[])
                .map_err(|e| DbError::Storage(format!("NebariDB meta_set: {}", e)))?;
            Ok(())
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))??;

        // Открываем дерево пользователя
        let tree = spawn_blocking(move || -> DbResult<Tree<UnversionedTreeRoot<()>, StdFile>> {
            roots
                .tree(Unversioned::tree(table_name))
                .map_err(|e| DbError::Storage(format!("NebariDB open_tree: {}", e)))
        })
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
    async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        let tree = self.tree.clone();
        let inner_value = value.clone();

        spawn_blocking(move || -> DbResult<RecordId> {
            let id = RecordId::new();
            let now = Utc::now().timestamp_micros() as u64;
            let record: RepoRecord = (id, now, now, inner_value);

            let key = id.as_bytes().to_vec();
            let serialized =
                rmp_serde::to_vec(&record).map_err(|e| DbError::Codec(e.to_string()))?;

            tree.set(key, serialized)
                .map_err(|e| DbError::Storage(format!("NebariDB set: {}", e)))?;

            Ok(id)
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordId, value: &InnerValue) -> DbResult<bool> {
        let tree = self.tree.clone();
        let inner_value = value.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let key_bytes = key.as_bytes().to_vec();

            let existing_val = tree
                .get(&key_bytes)
                .map_err(|e| DbError::Storage(format!("NebariDB get: {}", e)))?;

            let created_at = match existing_val {
                Some(v) => rmp_serde::from_slice::<RepoRecord>(v.as_ref())
                    .map(|r| r.1)
                    .unwrap_or_else(|_| Utc::now().timestamp_micros() as u64),
                None => Utc::now().timestamp_micros() as u64,
            };

            let record: RepoRecord = (
                key,
                created_at,
                Utc::now().timestamp_micros() as u64,
                inner_value,
            );

            let serialized =
                rmp_serde::to_vec(&record).map_err(|e| DbError::Codec(e.to_string()))?;

            tree.set(key_bytes, serialized)
                .map_err(|e| DbError::Storage(format!("NebariDB set: {}", e)))?;

            Ok(true)
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordId) -> DbResult<RepoRecord> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<RepoRecord> {
            let key_bytes = key.as_bytes();
            let val = tree
                .get(key_bytes)
                .map_err(|e| DbError::Storage(format!("NebariDB get: {}", e)))?
                .ok_or_else(|| DbError::NotFound(key.to_string()))?;

            rmp_serde::from_slice(val.as_ref()).map_err(|e| DbError::Codec(e.to_string()))
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn remove(&self, key: RecordId) -> DbResult<bool> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let key_bytes = key.as_bytes();
            let existed = tree
                .remove(key_bytes)
                .map_err(|e| DbError::Storage(format!("NebariDB remove: {}", e)))?
                .is_some();

            Ok(existed)
        })
            .await
            .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn iter(&self) -> DbResult<Vec<RepoRecord>> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<Vec<RepoRecord>> {
            let mut out = Vec::new();
            tree.scan::<nebari::Error, _, _, _, _>(
                &(..),
                true, // forward
                |_, _, _| ScanEvaluation::ReadData,
                |_, _| ScanEvaluation::ReadData,
                |_, _, value| {
                    let val = value.as_ref();
                    let record: RepoRecord = rmp_serde::from_slice(val).map_err(|e| {
                        nebari::Error::from(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            e.to_string(),
                        ))
                    })?;
                    out.push(record);
                    Ok(())
                },
            )
                .map_err(|e| DbError::Storage(format!("NebariDB scan: {}", e)))?;

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

        // Verify removal
        match store.get(id1).await {
            Err(DbError::NotFound(_)) => { /* Correct */ }
            Ok(_) => panic!("Should have been removed"),
            Err(e) => panic!("Unexpected error: {}", e),
        }

        assert_eq!(store.iter().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_nebari_repo_basic() {
        let path = "./test_data/nebari_repo_basic.nebari";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = NebariRepo::new(path).unwrap();
        let store = repo.store_get("test_table").await.unwrap();

        run_store_tests(store).await;
    }

    #[tokio::test]
    async fn test_nebari_repo_list_stores() {
        let path = "./test_data/nebari_repo_list.nebari";
        if std::path::Path::new(path).exists() {
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
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = NebariRepo::new(path).unwrap();

        let store1 = repo.store_get("isolated_table1").await.unwrap();
        let store2 = repo.store_get("isolated_table2").await.unwrap();

        // Insert into table1
        let value1 = InnerValue::Str("table1_value".to_string());
        let id1 = store1.insert(&value1).await.unwrap();

        // Insert into table2
        let value2 = InnerValue::Str("table2_value".to_string());
        let id2 = store2.insert(&value2).await.unwrap();

        // Verify isolation - each table should have only 1 record
        assert_eq!(store1.iter().await.unwrap().len(), 1);
        assert_eq!(store2.iter().await.unwrap().len(), 1);

        // Verify correct values
        let retrieved1 = store1.get(id1).await.unwrap();
        assert_eq!(retrieved1.3, value1);

        let retrieved2 = store2.get(id2).await.unwrap();
        assert_eq!(retrieved2.3, value2);

        // Verify cross-table isolation (get should fail with NotFound)
        assert!(matches!(store2.get(id1).await, Err(DbError::NotFound(_))));
        assert!(matches!(store1.get(id2).await, Err(DbError::NotFound(_))));

        // Clean up
        repo.store_delete("isolated_table1").await.unwrap();
        repo.store_delete("isolated_table2").await.unwrap();
    }
}